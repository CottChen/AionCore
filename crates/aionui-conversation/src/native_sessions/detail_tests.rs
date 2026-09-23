use super::*;
use sqlx::sqlite::SqliteConnectOptions;
#[tokio::test]
async fn opencode_equal_timestamp_parts_do_not_repeat_and_payloads_are_bounded() {
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().in_memory(true))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT)")
        .execute(&mut db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO message VALUES ('m','s','{\"role\":\"user\"}')")
        .execute(&mut db)
        .await
        .unwrap();
    for i in 0..25 {
        sqlx::query("INSERT INTO part VALUES (?, 's', 'm', 100, ?)")
            .bind(format!("part-{i:02}"))
            .bind(serde_json::json!({"type":"text","text":format!("item-{i}")}).to_string())
            .execute(&mut db)
            .await
            .unwrap();
    }
    let (first, next, _) = read_parts(&mut db, "s", None, 20).await.unwrap();
    let (last, next2, _) = read_parts(&mut db, "s", next.as_deref(), 20).await.unwrap();
    assert_eq!((first.len(), last.len()), (20, 5));
    assert!(next2.is_none());
    assert!(
        first
            .iter()
            .all(|a| a.role == "user" && last.iter().all(|b| a.id != b.id))
    );
    assert_eq!(
        read_parts(&mut db, "other", next.as_deref(), 20).await.unwrap_err(),
        "bad_request"
    );
    sqlx::query("INSERT INTO part VALUES ('large','s','m',101,?)")
        .bind("x".repeat(65536))
        .execute(&mut db)
        .await
        .unwrap();
    let (large, _, partial) = read_parts(&mut db, "s", None, 1).await.unwrap();
    assert!(partial);
    assert_eq!(large[0].kind, "oversized");
    assert!(large[0].text.is_empty());
}
#[tokio::test]
async fn client_cannot_supply_a_path_or_unbounded_page_size() {
    assert_eq!(
        get(NativeSessionBackend::Pi, "../secret", None, None)
            .await
            .unwrap_err(),
        "bad_request"
    );
    assert_eq!(
        get(NativeSessionBackend::Codex, "id", None, Some(10000))
            .await
            .unwrap_err(),
        "bad_request"
    );
    assert_eq!(
        get(NativeSessionBackend::Codex, "id", None, Some(0)).await.unwrap_err(),
        "bad_request"
    );
}
