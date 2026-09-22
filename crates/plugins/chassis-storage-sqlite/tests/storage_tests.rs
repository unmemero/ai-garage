use chassis_storage_sqlite::{ConversationStore, Role, StorageConfig};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn test_conversation_crud_and_pagination() {
    let store = ConversationStore::open_in_memory(384).await.unwrap();

    // 1. Create conversations
    let c1 = store
        .create_conversation(
            "conv_1",
            "First Chat",
            "meta-llama-3.1-8b",
            Some("You are a helpful assistant"),
            Some("ses_wal_1234"),
            Some(json!({ "category": "general" })),
        )
        .await
        .unwrap();

    assert_eq!(c1.id, "conv_1");
    assert_eq!(c1.title, "First Chat");
    assert_eq!(c1.chassis_session_id.as_deref(), Some("ses_wal_1234"));

    let _c2 = store
        .create_conversation("conv_2", "Second Chat", "gpt-4o", None, None, None)
        .await
        .unwrap();

    // 2. Get by ID
    let fetched = store.get_conversation("conv_1").await.unwrap().unwrap();
    assert_eq!(fetched.title, "First Chat");
    assert_eq!(fetched.metadata["category"], "general");

    // 3. List conversations
    let list = store.list_conversations(10, 0).await.unwrap();
    assert_eq!(list.len(), 2);

    // 4. Update title
    let updated = store
        .update_conversation_title("conv_1", "Updated First Chat")
        .await
        .unwrap();
    assert!(updated);

    let re_fetched = store.get_conversation("conv_1").await.unwrap().unwrap();
    assert_eq!(re_fetched.title, "Updated First Chat");

    // 5. Delete conversation
    let deleted = store.delete_conversation("conv_2").await.unwrap();
    assert!(deleted);

    let list_after = store.list_conversations(10, 0).await.unwrap();
    assert_eq!(list_after.len(), 1);
    assert_eq!(list_after[0].id, "conv_1");
}

#[tokio::test]
async fn test_messages_chronological_ordering_and_cascade_delete() {
    let store = ConversationStore::open_in_memory(384).await.unwrap();

    store
        .create_conversation("conv_test", "Thread", "llama", None, None, None)
        .await
        .unwrap();

    // Append 3 messages
    store
        .append_message(
            "m1",
            "conv_test",
            Role::User,
            "Hello world",
            5,
            Some(1),
            None,
            None,
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    store
        .append_message(
            "m2",
            "conv_test",
            Role::Assistant,
            "Hi there!",
            4,
            Some(2),
            None,
            None,
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    store
        .append_message(
            "m3",
            "conv_test",
            Role::Tool,
            "{\"result\":\"ok\"}",
            6,
            Some(3),
            None,
            None,
        )
        .await
        .unwrap();

    let messages = store.get_messages("conv_test", 10, 0).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id, "m1");
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].wal_seq, Some(1));
    assert_eq!(messages[1].id, "m2");
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[1].wal_seq, Some(2));
    assert_eq!(messages[2].id, "m3");
    assert_eq!(messages[2].role, Role::Tool);
    assert_eq!(messages[2].wal_seq, Some(3));

    // Cascade delete: deleting the conversation must delete its messages
    store.delete_conversation("conv_test").await.unwrap();
    let messages_after = store.get_messages("conv_test", 10, 0).await.unwrap();
    assert!(messages_after.is_empty());
}

#[tokio::test]
async fn test_semantic_vector_search_cosine_accuracy() {
    // 3-dimensional embeddings for clear geometric testing
    let store = ConversationStore::open_in_memory(3).await.unwrap();

    store
        .create_conversation("c1", "Tech & Weather", "model", None, None, None)
        .await
        .unwrap();

    // Message 1: Vector pointing along X-axis [1.0, 0.0, 0.0]
    store
        .append_message(
            "msg_microkernel",
            "c1",
            Role::User,
            "How do zero-trust microkernels work?",
            10,
            None,
            None,
            Some(&[1.0, 0.0, 0.0]),
        )
        .await
        .unwrap();

    // Message 2: Vector pointing along Y-axis [0.0, 1.0, 0.0] (orthogonal)
    store
        .append_message(
            "msg_weather",
            "c1",
            Role::Assistant,
            "It is raining in Seattle today.",
            8,
            None,
            None,
            Some(&[0.0, 1.0, 0.0]),
        )
        .await
        .unwrap();

    // Message 3: Vector close to X-axis [0.92, 0.08, 0.0]
    store
        .append_message(
            "msg_security",
            "c1",
            Role::User,
            "Explain memory safety in Rust.",
            9,
            None,
            None,
            Some(&[0.92, 0.08, 0.0]),
        )
        .await
        .unwrap();

    // Query vector: [0.98, 0.02, 0.0] (very close to X-axis)
    let matches = store
        .search_similar_messages(&[0.98, 0.02, 0.0], 2, None)
        .await
        .unwrap();

    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].message.id, "msg_microkernel");
    assert_eq!(matches[1].message.id, "msg_security");
    assert!(
        matches[0].distance < matches[1].distance,
        "msg_microkernel should have smaller cosine distance"
    );
}

#[tokio::test]
async fn test_file_based_persistence_and_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("conversations.db");

    // Phase 1: Open, write conversation + message with vector, and drop
    {
        let store = ConversationStore::open_local(&db_path, 4).await.unwrap();
        store
            .create_conversation("persisted_c1", "Persistent Thread", "model", None, None, None)
            .await
            .unwrap();

        store
            .append_message(
                "p_m1",
                "persisted_c1",
                Role::User,
                "Persistent memory test",
                5,
                Some(42),
                None,
                Some(&[0.5, 0.5, 0.5, 0.5]),
            )
            .await
            .unwrap();
    }

    // Phase 2: Reopen from disk and verify data + vector search intact
    {
        let store = ConversationStore::open(StorageConfig {
            db_path: Some(db_path),
            vector_dimensions: 4,
        })
        .await
        .unwrap();

        let conv = store
            .get_conversation("persisted_c1")
            .await
            .unwrap()
            .expect("Conversation must persist");
        assert_eq!(conv.title, "Persistent Thread");

        let messages = store.get_messages("persisted_c1", 10, 0).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Persistent memory test");
        assert_eq!(messages[0].wal_seq, Some(42));

        let matches = store
            .search_similar_messages(&[0.5, 0.5, 0.5, 0.5], 1, None)
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].message.id, "p_m1");
    }
}
