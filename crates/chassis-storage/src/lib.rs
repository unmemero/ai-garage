pub mod models;

use chrono::Utc;
use libsql::{params, Builder, Connection, Database};
pub use models::{Conversation, Message, Role, SimilarityMatch};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Database(#[from] libsql::Error),

    #[error("Vector dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Entity not found: {0}")]
    NotFound(String),
}

/// Configuration for the conversation database
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Path to local sqlite file. If None, runs in-memory (`:memory:`).
    pub db_path: Option<PathBuf>,
    /// Dimensionality of the vector embeddings (e.g. 384, 768, 1536)
    pub vector_dimensions: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            vector_dimensions: 384,
        }
    }
}

/// Sovereign Conversation Storage and Vector Memory Engine backed by libSQL/Turso
#[derive(Clone)]
pub struct ConversationStore {
    _db: Arc<Database>,
    conn: Arc<Connection>,
    dimensions: usize,
}

impl ConversationStore {
    /// Open or create an in-memory conversation database
    pub async fn open_in_memory(dimensions: usize) -> Result<Self, StorageError> {
        let config = StorageConfig {
            db_path: None,
            vector_dimensions: dimensions,
        };
        Self::open(config).await
    }

    /// Open or create a local file-based conversation database
    pub async fn open_local(
        path: impl AsRef<Path>,
        dimensions: usize,
    ) -> Result<Self, StorageError> {
        let config = StorageConfig {
            db_path: Some(path.as_ref().to_path_buf()),
            vector_dimensions: dimensions,
        };
        Self::open(config).await
    }

    /// Open with custom configuration
    pub async fn open(config: StorageConfig) -> Result<Self, StorageError> {
        if let Some(parent) = config.db_path.as_ref().and_then(|p| p.parent()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                libsql::Error::Misuse(format!("Failed to create storage directory: {e}"))
            })?;
        }

        let db = match &config.db_path {
            Some(path) => {
                let path_str = path.to_str().ok_or_else(|| {
                    libsql::Error::Misuse("Invalid non-UTF8 database path".into())
                })?;
                Builder::new_local(path_str).build().await?
            }
            None => Builder::new_local(":memory:").build().await?,
        };

        let conn = db.connect()?;

        // Enable foreign key enforcement
        conn.execute("PRAGMA foreign_keys = ON;", ()).await?;

        // 1. Create conversations table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                model_id TEXT NOT NULL,
                system_prompt TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                chassis_session_id TEXT,
                metadata JSON NOT NULL DEFAULT '{}'
            );",
            (),
        )
        .await?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_conversations_updated 
             ON conversations(updated_at DESC);",
            (),
        )
        .await?;

        // 2. Create messages table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                token_count INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                wal_seq INTEGER,
                metadata JSON NOT NULL DEFAULT '{}'
            );",
            (),
        )
        .await?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_conv_created 
             ON messages(conversation_id, created_at ASC);",
            (),
        )
        .await?;

        // 3. Create message embeddings table with native vector column
        let create_vec_table_sql = format!(
            "CREATE TABLE IF NOT EXISTS message_embeddings (
                message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
                conversation_id TEXT NOT NULL,
                embedding F32_BLOB({})
            );",
            config.vector_dimensions
        );
        conn.execute(&create_vec_table_sql, ()).await?;

        // 4. Create native ANN vector index with cosine similarity
        let create_vec_idx_sql = "CREATE INDEX IF NOT EXISTS idx_message_embeddings_vec 
             ON message_embeddings (libsql_vector_idx(embedding, 'metric=cosine'));";
        conn.execute(create_vec_idx_sql, ()).await?;

        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(conn),
            dimensions: config.vector_dimensions,
        })
    }

    /// Vector dimension size configured for this store
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    // =========================================================================
    // Conversation Management
    // =========================================================================

    /// Create a new conversation thread
    pub async fn create_conversation(
        &self,
        id: &str,
        title: &str,
        model_id: &str,
        system_prompt: Option<&str>,
        chassis_session_id: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Conversation, StorageError> {
        let now = Utc::now().timestamp_millis();
        let meta_val = metadata.unwrap_or_else(|| serde_json::json!({}));
        let meta_str = meta_val.to_string();

        self.conn
            .execute(
                "INSERT INTO conversations (id, title, model_id, system_prompt, created_at, updated_at, chassis_session_id, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8);",
                params![
                    id,
                    title,
                    model_id,
                    system_prompt,
                    now,
                    now,
                    chassis_session_id,
                    meta_str
                ],
            )
            .await?;

        Ok(Conversation {
            id: id.to_string(),
            title: title.to_string(),
            model_id: model_id.to_string(),
            system_prompt: system_prompt.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
            chassis_session_id: chassis_session_id.map(|s| s.to_string()),
            metadata: meta_val,
        })
    }

    /// Retrieve a conversation by ID
    pub async fn get_conversation(
        &self,
        id: &str,
    ) -> Result<Option<Conversation>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, model_id, system_prompt, created_at, updated_at, chassis_session_id, metadata
                 FROM conversations WHERE id = ?1;",
                params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let meta_str: String = row.get(7)?;
            Ok(Some(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                model_id: row.get(2)?,
                system_prompt: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                chassis_session_id: row.get(6)?,
                metadata: serde_json::from_str(&meta_str)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// List conversations ordered by last updated
    pub async fn list_conversations(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Conversation>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, title, model_id, system_prompt, created_at, updated_at, chassis_session_id, metadata
                 FROM conversations ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2;",
                params![limit as i64, offset as i64],
            )
            .await?;

        let mut list = Vec::new();
        while let Some(row) = rows.next().await? {
            let meta_str: String = row.get(7)?;
            list.push(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                model_id: row.get(2)?,
                system_prompt: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                chassis_session_id: row.get(6)?,
                metadata: serde_json::from_str(&meta_str)?,
            });
        }
        Ok(list)
    }

    /// Update conversation title
    pub async fn update_conversation_title(
        &self,
        id: &str,
        new_title: &str,
    ) -> Result<bool, StorageError> {
        let now = Utc::now().timestamp_millis();
        let affected = self
            .conn
            .execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3;",
                params![new_title, now, id],
            )
            .await?;
        Ok(affected > 0)
    }

    /// Delete a conversation (cascades automatically to messages and embeddings)
    pub async fn delete_conversation(&self, id: &str) -> Result<bool, StorageError> {
        let affected = self
            .conn
            .execute("DELETE FROM conversations WHERE id = ?1;", params![id])
            .await?;
        Ok(affected > 0)
    }

    // =========================================================================
    // Message Operations
    // =========================================================================

    /// Append a new message turn to a conversation, optionally storing its vector embedding
    #[allow(clippy::too_many_arguments)]
    pub async fn append_message(
        &self,
        id: &str,
        conversation_id: &str,
        role: Role,
        content: &str,
        token_count: i64,
        wal_seq: Option<i64>,
        metadata: Option<serde_json::Value>,
        embedding: Option<&[f32]>,
    ) -> Result<Message, StorageError> {
        if let Some(emb) = embedding {
            if emb.len() != self.dimensions {
                return Err(StorageError::DimensionMismatch {
                    expected: self.dimensions,
                    got: emb.len(),
                });
            }
        }

        let now = Utc::now().timestamp_millis();
        let meta_val = metadata.unwrap_or_else(|| serde_json::json!({}));
        let meta_str = meta_val.to_string();

        // 1. Insert message
        self.conn
            .execute(
                "INSERT INTO messages (id, conversation_id, role, content, token_count, created_at, wal_seq, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8);",
                params![
                    id,
                    conversation_id,
                    role.as_str(),
                    content,
                    token_count,
                    now,
                    wal_seq,
                    meta_str
                ],
            )
            .await?;

        // 2. Insert embedding vector if provided
        if let Some(emb) = embedding {
            let vec_str = format_vector_sql(emb);
            let insert_vec_sql = format!(
                "INSERT INTO message_embeddings (message_id, conversation_id, embedding)
                 VALUES ('{}', '{}', vector('{}'));",
                id, conversation_id, vec_str
            );
            self.conn.execute(&insert_vec_sql, ()).await?;
        }

        // 3. Touch conversation updated_at
        self.conn
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2;",
                params![now, conversation_id],
            )
            .await?;

        Ok(Message {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            content: content.to_string(),
            token_count,
            created_at: now,
            wal_seq,
            metadata: meta_val,
        })
    }

    /// Retrieve messages for a conversation ordered chronologically
    pub async fn get_messages(
        &self,
        conversation_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Message>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT id, conversation_id, role, content, token_count, created_at, wal_seq, metadata
                 FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3;",
                params![conversation_id, limit as i64, offset as i64],
            )
            .await?;

        let mut list = Vec::new();
        while let Some(row) = rows.next().await? {
            let role_str: String = row.get(2)?;
            let role = Role::from_str(&role_str).unwrap_or(Role::User);
            let meta_str: String = row.get(7)?;
            list.push(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role,
                content: row.get(3)?,
                token_count: row.get(4)?,
                created_at: row.get(5)?,
                wal_seq: row.get(6)?,
                metadata: serde_json::from_str(&meta_str)?,
            });
        }
        Ok(list)
    }

    // =========================================================================
    // Semantic Memory & Vector Search
    // =========================================================================

    /// Search for semantically similar messages across all conversations or filtered to a single thread
    pub async fn search_similar_messages(
        &self,
        query_vector: &[f32],
        top_k: usize,
        filter_conversation_id: Option<&str>,
    ) -> Result<Vec<SimilarityMatch>, StorageError> {
        if query_vector.len() != self.dimensions {
            return Err(StorageError::DimensionMismatch {
                expected: self.dimensions,
                got: query_vector.len(),
            });
        }

        let vec_str = format_vector_sql(query_vector);

        let query_sql = match filter_conversation_id {
            Some(conv_id) => format!(
                "SELECT m.id, m.conversation_id, m.role, m.content, m.token_count, m.created_at, m.wal_seq, m.metadata,
                        vector_distance_cos(me.embedding, vector('{}')) AS distance
                 FROM vector_top_k('idx_message_embeddings_vec', vector('{}'), {}) AS top
                 JOIN message_embeddings me ON me.rowid = top.id
                 JOIN messages m ON m.id = me.message_id
                 WHERE me.conversation_id = '{}'
                 ORDER BY distance ASC;",
                vec_str, vec_str, top_k, conv_id
            ),
            None => format!(
                "SELECT m.id, m.conversation_id, m.role, m.content, m.token_count, m.created_at, m.wal_seq, m.metadata,
                        vector_distance_cos(me.embedding, vector('{}')) AS distance
                 FROM vector_top_k('idx_message_embeddings_vec', vector('{}'), {}) AS top
                 JOIN message_embeddings me ON me.rowid = top.id
                 JOIN messages m ON m.id = me.message_id
                 ORDER BY distance ASC;",
                vec_str, vec_str, top_k
            ),
        };

        let mut rows = self.conn.query(&query_sql, ()).await?;
        let mut matches = Vec::new();

        while let Some(row) = rows.next().await? {
            let role_str: String = row.get(2)?;
            let role = Role::from_str(&role_str).unwrap_or(Role::User);
            let meta_str: String = row.get(7)?;
            let distance: f64 = row.get(8)?;

            let message = Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role,
                content: row.get(3)?,
                token_count: row.get(4)?,
                created_at: row.get(5)?,
                wal_seq: row.get(6)?,
                metadata: serde_json::from_str(&meta_str)?,
            };

            matches.push(SimilarityMatch { message, distance });
        }

        Ok(matches)
    }
}

fn format_vector_sql(vec: &[f32]) -> String {
    let mut s = String::with_capacity(vec.len() * 8 + 2);
    s.push('[');
    for (i, val) in vec.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&val.to_string());
    }
    s.push(']');
    s
}
