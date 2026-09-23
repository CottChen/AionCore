use super::*;

/// Manual smoke check only: explicitly opt in to reading host CLI metadata.
#[tokio::test]
#[ignore = "requires AIONUI_NATIVE_SESSIONS_WORKSPACE and local CLI storage"]
async fn read_host_catalogs_with_explicit_workspace() {
    let workspace = std::env::var("AIONUI_NATIVE_SESSIONS_WORKSPACE").expect("select a workspace explicitly");
    for backend in [
        NativeSessionBackend::Codex,
        NativeSessionBackend::Pi,
        NativeSessionBackend::Opencode,
    ] {
        let root = native_root(backend).expect("home directory");
        let (items, status) = read_catalog(&root, backend, &workspace, None).await;
        println!("{backend:?}: status={status}, visible_items={}", items.len());
        assert!(matches!(status.as_str(), "ok" | "missing"));
        assert!(items.iter().all(|item| item.workspace == workspace));
        if backend == NativeSessionBackend::Codex
            && let Ok(expected) = std::env::var("AIONUI_EXPECT_NATIVE_ID")
        {
            assert!(items.iter().any(|item| item.id == expected));
        }
    }
}

#[tokio::test]
async fn sqlite_catalog_is_project_scoped_paginated_and_read_only() {
    let root = tempfile::tempdir().unwrap();
    for backend in [NativeSessionBackend::Codex, NativeSessionBackend::Opencode] {
        let (file, table, cwd, time) = if backend == NativeSessionBackend::Codex {
            ("state_5.sqlite", "threads", "cwd", "updated_at")
        } else {
            ("opencode.db", "session", "directory", "time_updated")
        };
        let options = SqliteConnectOptions::new()
            .filename(root.path().join(file))
            .create_if_missing(true);
        let mut db = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query(&format!(
            "CREATE TABLE {table} (id TEXT PRIMARY KEY, title TEXT, {cwd} TEXT, {time} INTEGER)"
        ))
        .execute(&mut db)
        .await
        .unwrap();
        for i in 0..25 {
            sqlx::query(&format!("INSERT INTO {table} VALUES (?, ?, ?, ?)"))
                .bind(format!("session-{i:02}"))
                .bind("Title")
                .bind("/project")
                .bind(1000i64)
                .execute(&mut db)
                .await
                .unwrap();
        }
        sqlx::query(&format!(
            "INSERT INTO {table} VALUES ('other', 'PRIVATE', '/other', 2000)"
        ))
        .execute(&mut db)
        .await
        .unwrap();
        db.close().await.unwrap();
        let before = std::fs::read(root.path().join(file)).unwrap();
        let (mut first, status) = read_catalog(root.path(), backend, "/project", None).await;
        assert_eq!(status, "ok");
        let cursor = page(&mut first, backend, "/project").unwrap();
        assert_eq!(first.len(), PAGE_SIZE);
        assert!(first.iter().all(|s| s.workspace == "/project" && s.id != "other"));
        let decoded = decode_cursor(Some(&cursor), backend, "/project").unwrap();
        let (second, _) = read_catalog(root.path(), backend, "/project", decoded.as_ref()).await;
        assert_eq!(second.len(), 5);
        assert!(second.iter().all(|s| !first.iter().any(|f| f.id == s.id)));
        assert_eq!(std::fs::read(root.path().join(file)).unwrap(), before);
        assert!(decode_cursor(Some(&cursor), backend, "/other").is_err());
    }
}

#[tokio::test]
async fn missing_and_incompatible_databases_have_explicit_status() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        read_catalog(root.path(), NativeSessionBackend::Opencode, "/project", None)
            .await
            .1,
        "missing"
    );
    let mut db = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(root.path().join("opencode.db"))
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TABLE other (id TEXT)")
        .execute(&mut db)
        .await
        .unwrap();
    db.close().await.unwrap();
    assert_eq!(
        read_catalog(root.path(), NativeSessionBackend::Opencode, "/project", None)
            .await
            .1,
        "unsupported_schema"
    );
    assert!(decode_cursor(Some("invalid!"), NativeSessionBackend::Pi, "/project").is_err());
}

#[tokio::test]
async fn invalid_storage_is_reported_as_read_error_not_missing() {
    let root = tempfile::tempdir().unwrap();
    let not_a_directory = root.path().join("file");
    std::fs::write(&not_a_directory, "not a directory").unwrap();
    assert_eq!(
        read_catalog(&not_a_directory, NativeSessionBackend::Codex, "/project", None)
            .await
            .1,
        "read_error"
    );
    std::fs::write(root.path().join("opencode.db"), "invalid database").unwrap();
    assert_eq!(
        read_catalog(root.path(), NativeSessionBackend::Opencode, "/project", None)
            .await
            .1,
        "read_error"
    );
}

#[tokio::test]
async fn pi_reads_only_matching_headers_and_reports_corruption_without_modifying_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(pi_project_dir("/project"));
    std::fs::create_dir(&dir).unwrap();
    let content = "{\"type\":\"session\",\"id\":\"pi-id\",\"cwd\":\"/project\"}\n{\"type\":\"session_info\",\"name\":\"My session\"}\n";
    std::fs::write(dir.join("one.jsonl"), content).unwrap();
    std::fs::write(
        dir.join("collision.jsonl"),
        "{\"type\":\"session\",\"id\":\"other\",\"cwd\":\"/different\"}\n",
    )
    .unwrap();
    std::fs::write(dir.join("broken.jsonl"), "not json").unwrap();
    let (items, status) = read_catalog(root.path(), NativeSessionBackend::Pi, "/project", None).await;
    assert_eq!(status, "partial");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "pi-id");
    assert_eq!(items[0].title, "My session");
    assert_eq!(std::fs::read_to_string(dir.join("one.jsonl")).unwrap(), content);
}
