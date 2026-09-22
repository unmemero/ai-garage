use serde::{Deserialize, Serialize};

/// Role of a message participant
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            "system" => Ok(Role::System),
            "tool" => Ok(Role::Tool),
            other => Err(format!("Unknown role: {other}")),
        }
    }
}

/// A conversation or chat thread
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub model_id: String,
    pub system_prompt: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Associated Chassis sovereign session ID for cryptographic WAL auditability
    pub chassis_session_id: Option<String>,
    pub metadata: serde_json::Value,
}

/// An individual message turn within a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: Role,
    pub content: String,
    pub token_count: i64,
    pub created_at: i64,
    /// Exact sequence number in Chassis Write-Ahead Log if executed
    pub wal_seq: Option<i64>,
    pub metadata: serde_json::Value,
}

/// Result of a vector similarity search across message memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityMatch {
    pub message: Message,
    /// Cosine distance (smaller = closer similarity)
    pub distance: f64,
}
