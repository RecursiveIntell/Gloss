use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, Role};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, tmp)
}

#[tokio::test]
async fn create_session_and_list() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("repl").await.unwrap();
    assert!(!sid.is_empty());

    let sessions = store.list_sessions(10, 0).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, sid);
    assert_eq!(sessions[0].channel, "repl");
    assert_eq!(sessions[0].message_count, 0);
}

#[tokio::test]
async fn add_messages_and_retrieve() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    store
        .add_message(&sid, Role::User, "Hello", Some(10), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::Assistant, "Hi there!", Some(15), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "How are you?", Some(12), None)
        .await
        .unwrap();

    let messages = store.get_recent_messages(&sid, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[2].content, "How are you?");
}

#[tokio::test]
async fn get_recent_messages_with_limit() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    for i in 0..5 {
        store
            .add_message(&sid, Role::User, &format!("Message {}", i), Some(10), None)
            .await
            .unwrap();
    }

    let messages = store.get_recent_messages(&sid, 3).await.unwrap();
    assert_eq!(messages.len(), 3);
    // Should be the last 3 in chronological order
    assert_eq!(messages[0].content, "Message 2");
    assert_eq!(messages[1].content, "Message 3");
    assert_eq!(messages[2].content, "Message 4");
}

#[tokio::test]
async fn token_budget_limiting() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    for i in 0..5 {
        store
            .add_message(&sid, Role::User, &format!("Msg {}", i), Some(100), None)
            .await
            .unwrap();
    }

    // Budget of 250 should fit at most 2 messages
    let messages = store.get_messages_within_budget(&sid, 250).await.unwrap();
    assert_eq!(messages.len(), 2);
    // Should be the last 2 in chronological order
    assert_eq!(messages[0].content, "Msg 3");
    assert_eq!(messages[1].content, "Msg 4");
}

#[tokio::test]
async fn token_budget_null_tokens_always_included() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    store
        .add_message(&sid, Role::User, "No tokens", None, None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "Has tokens", Some(100), None)
        .await
        .unwrap();

    // Both should be included since auto-computed token count is small
    let messages = store.get_messages_within_budget(&sid, 200).await.unwrap();
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn session_token_count() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    store
        .add_message(&sid, Role::User, "A", Some(10), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "B", Some(20), None)
        .await
        .unwrap();
    // None token_count is auto-computed (1 char / 4 = max(1,0) = 1)
    store
        .add_message(&sid, Role::User, "C", None, None)
        .await
        .unwrap();

    let count = store.session_token_count(&sid).await.unwrap();
    // 10 + 20 + 1 (auto-computed for "C") = 31
    assert_eq!(count, 31);
}

#[tokio::test]
async fn session_isolation() {
    let (store, _tmp) = test_store();
    let sid_a = store.create_session("a").await.unwrap();
    let sid_b = store.create_session("b").await.unwrap();

    store
        .add_message(&sid_a, Role::User, "In session A", Some(10), None)
        .await
        .unwrap();
    store
        .add_message(&sid_b, Role::User, "In session B", Some(10), None)
        .await
        .unwrap();

    let msgs_a = store.get_recent_messages(&sid_a, 10).await.unwrap();
    let msgs_b = store.get_recent_messages(&sid_b, 10).await.unwrap();

    assert_eq!(msgs_a.len(), 1);
    assert_eq!(msgs_a[0].content, "In session A");
    assert_eq!(msgs_b.len(), 1);
    assert_eq!(msgs_b[0].content, "In session B");
}

#[tokio::test]
async fn delete_session_cascades_messages() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    store
        .add_message(&sid, Role::User, "Hello", Some(5), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "World", Some(5), None)
        .await
        .unwrap();

    store.delete_session(&sid).await.unwrap();

    // Session should be gone
    let sessions = store.list_sessions(10, 0).await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn delete_nonexistent_session_returns_error() {
    let (store, _tmp) = test_store();
    let result = store.delete_session("nonexistent-uuid").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn add_message_to_nonexistent_session_returns_error() {
    let (store, _tmp) = test_store();
    let result = store
        .add_message("nonexistent-uuid", Role::User, "Hello", None, None)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn message_count_on_list_sessions() {
    let (store, _tmp) = test_store();
    let sid = store.create_session("test").await.unwrap();

    store
        .add_message(&sid, Role::User, "A", Some(5), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "B", Some(5), None)
        .await
        .unwrap();
    store
        .add_message(&sid, Role::User, "C", Some(5), None)
        .await
        .unwrap();

    let sessions = store.list_sessions(10, 0).await.unwrap();
    assert_eq!(sessions[0].message_count, 3);
}
