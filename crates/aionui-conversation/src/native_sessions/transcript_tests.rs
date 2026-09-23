use super::*;
use serde_json::json;
fn fixture(count: usize) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session.jsonl");
    let mut text = json!({"type":"session","id":"pi-id","cwd":"/workspace"}).to_string() + "\n";
    for i in 0..count {
        text += &(json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":format!("message-{i}")}]}}).to_string()+"\n");
    }
    std::fs::write(&path, text).unwrap();
    (root, path)
}
#[test]
fn newest_first_pages_are_disjoint_and_keep_chronological_order_inside_each_page() {
    let (r, p) = fixture(45);
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for expected in [25, 5, 0] {
        let (messages, next, partial) =
            page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", cursor.as_deref(), 20).unwrap();
        assert!(!partial);
        assert_eq!(messages[0].text, format!("message-{expected}"));
        for m in messages {
            assert!(seen.insert(m.id));
        }
        cursor = next;
    }
    assert!(cursor.is_none());
    assert_eq!(seen.len(), 45);
}
#[test]
fn huge_lines_advance_in_bounded_windows_and_preserve_older_messages() {
    let (r, p) = fixture(1);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    writeln!(
        f,
        "{{\"type\":\"ignored\",\"huge\":\"{}\"}}",
        "x".repeat(READ_BYTES * 3)
    )
    .unwrap();
    drop(f);
    let mut cursor = None;
    let mut found = false;
    let mut steps = 0;
    loop {
        let (messages, next, _) = page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", cursor.as_deref(), 20).unwrap();
        found |= messages.iter().any(|m| m.text == "message-0");
        steps += 1;
        if next.is_none() {
            break;
        }
        assert_ne!(next, cursor);
        cursor = next;
        assert!(steps < 10);
    }
    assert!(found);
    assert!(steps >= 3);
}
#[test]
fn cursor_is_session_bound_and_detects_replaced_files_but_accepts_appends() {
    let (r, p) = fixture(30);
    let (_, next, _) = page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap();
    assert_eq!(
        page(r.path(), &p, NativeSessionBackend::Pi, "other", next.as_deref(), 20).unwrap_err(),
        "bad_request"
    );
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().append(true).open(&p).unwrap(), "{{}}").unwrap();
    assert!(page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", next.as_deref(), 20).is_ok());
    std::fs::write(&p, "{}\n").unwrap();
    assert_eq!(
        page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", next.as_deref(), 20).unwrap_err(),
        "stale_cursor"
    );
}
#[test]
fn invalid_records_and_oversized_display_text_are_explicit() {
    let (r, p) = fixture(0);
    let v = json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"中".repeat(TEXT_CHARS+1)}]}});
    std::fs::write(&p, format!("broken\n{v}\n")).unwrap();
    let (messages, _, partial) = page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap();
    assert!(partial);
    assert_eq!(messages[0].kind, "malformed");
    assert_eq!(messages[1].text.chars().count(), TEXT_CHARS);
    assert!(messages[1].truncated);
}
#[test]
fn completed_eof_record_needs_no_newline_but_incomplete_appends_are_skipped() {
    let (r, p) = fixture(2);
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.trim_end()).unwrap();
    let (messages, _, partial) = page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(!partial);
    std::fs::write(&p, format!("{text}{{\"type\":\"message\"")).unwrap();
    let (messages, _, partial) = page(r.path(), &p, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(partial);
}

#[test]
fn codex_keeps_one_copy_of_messages_and_includes_tool_records() {
    let event = json!({"type":"event_msg","payload":{"type":"agent_message","message":"duplicate"}});
    assert!(parse(&event, NativeSessionBackend::Codex).is_none());
    let call = json!({"type":"response_item","payload":{"type":"function_call","name":"read","arguments":"{\"path\":\"file\"}"}});
    assert_eq!(parse(&call, NativeSessionBackend::Codex).unwrap().kind, "tool_call");
}
#[cfg(unix)]
#[test]
fn refuses_symlinks_and_paths_outside_the_cli_root() {
    let (r, p) = fixture(1);
    let elsewhere = tempfile::tempdir().unwrap();
    let link = r.path().join("link.jsonl");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert_eq!(
        page(r.path(), &link, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap_err(),
        "read_error"
    );
    assert_eq!(
        page(elsewhere.path(), &p, NativeSessionBackend::Pi, "pi-id", None, 20).unwrap_err(),
        "read_error"
    );
}
