use super::*;
#[tokio::test]
async fn global_catalog_pages_are_read_only_and_optional_metadata_is_not_required() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state_5.sqlite");
    let mut db = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path).create_if_missing(true))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, cwd TEXT, updated_at INTEGER)")
        .execute(&mut db)
        .await
        .unwrap();
    for i in 0..25 {
        sqlx::query("INSERT INTO threads VALUES (?, 'title', '/work', 1000)")
            .bind(format!("id-{i:02}"))
            .execute(&mut db)
            .await
            .unwrap();
    }
    db.close().await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let (first, next, _) = list_sqlite(root.path(), NativeSessionBackend::Codex, None, "")
        .await
        .unwrap();
    let (second, next2, _) = list_sqlite(root.path(), NativeSessionBackend::Codex, next.as_ref(), "")
        .await
        .unwrap();
    assert_eq!((first.len(), second.len()), (20, 5));
    assert!(next2.is_none());
    assert!(
        first
            .iter()
            .all(|a| second.iter().all(|b| a.id != b.id) && a.model.is_none())
    );
    let (literal, _, _) = list_sqlite(root.path(), NativeSessionBackend::Codex, None, "%")
        .await
        .unwrap();
    assert!(literal.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
#[test]
fn pi_catalog_reads_only_shallow_regular_files_and_cursor_stays_on_the_same_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("project");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..25 {
        std::fs::write(dir.join(format!("{i}.jsonl")),format!("{{\"type\":\"session\",\"id\":\"pi-{i}\",\"cwd\":\"/project\"}}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"Hello\"}}}}\n")).unwrap();
    }
    let (first, next, _) = list_pi(root.path(), None, "").unwrap();
    let (second, next2, _) = list_pi(root.path(), next.as_ref(), "").unwrap();
    assert_eq!((first.len(), second.len()), (20, 5));
    assert!(next2.is_none());
    assert!(
        first
            .iter()
            .all(|a| a.title == "Hello" && second.iter().all(|b| a.id != b.id))
    );
}
#[cfg(unix)]
#[test]
fn pi_metadata_rechecks_a_cached_file_before_opening_it() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = root.path().join("cached.jsonl");
    let target = outside.path().join("outside.jsonl");
    std::fs::write(&target, "{}\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let entry = pi_files::Entry {
        path,
        item: NativeSessionItem {
            id: "id".into(),
            title: String::new(),
            workspace: "/work".into(),
            updated_at: 0,
            created_at: None,
            model: None,
        },
    };
    assert_eq!(pi_files::enrich(root.path(), &entry).unwrap_err(), "read_error");
}

#[tokio::test]
async fn invalid_and_cross_query_cursors_are_rejected_before_reading_the_host() {
    assert_eq!(
        list(NativeSessionBackend::Pi, Some("invalid"), None).await.unwrap_err(),
        "bad_request"
    );
    let raw = encode(&Cursor {
        backend: NativeSessionBackend::Codex,
        search: "one".into(),
        position: Position::default(),
    });
    assert_eq!(
        list(NativeSessionBackend::Pi, Some(&raw), Some("one"))
            .await
            .unwrap_err(),
        "bad_request"
    );
    assert_eq!(
        list(NativeSessionBackend::Codex, Some(&raw), Some("two"))
            .await
            .unwrap_err(),
        "bad_request"
    );
}
