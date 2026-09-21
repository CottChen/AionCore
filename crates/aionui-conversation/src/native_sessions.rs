//! Read-only, project-scoped CLI session catalogs. No CLI process is spawned.
use std::path::{Path, PathBuf};
use std::time::Duration;

use aionui_api_types::{NativeSessionBackend, NativeSessionItem};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};

use crate::ConversationError;

pub(crate) const PAGE_SIZE: usize = 20;
static CATALOG_READS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Cursor {
    backend: NativeSessionBackend,
    workspace: String,
    updated_at: i64,
    id: String,
}

pub(crate) fn decode_cursor(
    raw: Option<&str>,
    backend: NativeSessionBackend,
    workspace: &str,
) -> Result<Option<Cursor>, ConversationError> {
    let Some(raw) = raw else { return Ok(None) };
    let invalid = || ConversationError::bad_request("Invalid native session cursor");
    if raw.len() > 8192 {
        return Err(invalid());
    }
    let bytes = URL_SAFE_NO_PAD.decode(raw).map_err(|_| invalid())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if cursor.backend != backend || cursor.workspace != workspace || cursor.id.is_empty() {
        return Err(invalid());
    }
    Ok(Some(cursor))
}

pub(crate) fn page(
    items: &mut Vec<NativeSessionItem>,
    backend: NativeSessionBackend,
    workspace: &str,
) -> Option<String> {
    if items.len() <= PAGE_SIZE {
        return None;
    }
    items.truncate(PAGE_SIZE);
    let last = items.last()?;
    let cursor = Cursor {
        backend,
        workspace: workspace.to_owned(),
        updated_at: last.updated_at,
        id: last.id.clone(),
    };
    Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).ok()?))
}

pub(crate) fn native_root(backend: NativeSessionBackend) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(match backend {
        NativeSessionBackend::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex")),
        NativeSessionBackend::Pi => std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi/agent"))
            .join("sessions"),
        NativeSessionBackend::Opencode => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("opencode"),
    })
}

pub(crate) async fn read_catalog(
    root: &Path,
    backend: NativeSessionBackend,
    workspace: &str,
    cursor: Option<&Cursor>,
) -> (Vec<NativeSessionItem>, String) {
    let permit = match tokio::time::timeout(Duration::from_secs(1), CATALOG_READS.clone().acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        _ => return (vec![], "read_error".into()),
    };
    if backend == NativeSessionBackend::Pi {
        let root = root.to_owned();
        let workspace = workspace.to_owned();
        let after = cursor.map(|c| (c.updated_at, c.id.clone()));
        return tokio::task::spawn_blocking(move || {
            let _permit = permit;
            read_pi(&root, &workspace, after.as_ref())
        })
        .await
        .unwrap_or_else(|_| (vec![], "read_error".into()));
    }
    let path = if backend == NativeSessionBackend::Codex {
        // Codex versions keep a versioned state index. Only inspect its direct directory.
        let mut entries = match tokio::fs::read_dir(root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (vec![], "missing".into()),
            Err(_) => return (vec![], "read_error".into()),
        };
        let mut databases = Vec::new();
        let mut scanned = 0;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => return (vec![], "read_error".into()),
            };
            scanned += 1;
            if scanned > 512 {
                return (vec![], "partial".into());
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(version) = name
                .strip_prefix("state_")
                .and_then(|s| s.strip_suffix(".sqlite"))
                .and_then(|s| s.parse::<u32>().ok())
            {
                databases.push((version, entry.path()));
            }
        }
        databases.sort_by_key(|(version, _)| *version);
        let Some((_, path)) = databases.pop() else {
            return (vec![], "missing".into());
        };
        path
    } else {
        root.join("opencode.db")
    };
    match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (vec![], "missing".into()),
        _ => return (vec![], "read_error".into()),
    }
    let operation = async {
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(1));
        let mut db = SqliteConnection::connect_with(&options).await?;
        let (table, cwd, updated, scale) = if backend == NativeSessionBackend::Codex {
            ("threads", "cwd", "updated_at", 1000i64)
        } else {
            ("session", "directory", "time_updated", 1i64)
        };
        let (time, id) = cursor
            .map(|c| (c.updated_at / scale, c.id.as_str()))
            .unwrap_or((i64::MAX / scale, ""));
        let sql = format!(
            "SELECT id, substr(title,1,240) AS title, {updated} AS updated FROM {table} WHERE {cwd} = ? AND ({updated} < ? OR ({updated} = ? AND id < ?)) ORDER BY {updated} DESC, id DESC LIMIT ?"
        );
        let rows = sqlx::query(&sql)
            .bind(workspace)
            .bind(time)
            .bind(time)
            .bind(id)
            .bind((PAGE_SIZE + 1) as i64)
            .fetch_all(&mut db)
            .await?;
        let items = rows
            .into_iter()
            .map(|row| {
                Ok(NativeSessionItem {
                    id: row.try_get("id")?,
                    title: row.try_get("title")?,
                    workspace: workspace.to_owned(),
                    updated_at: row.try_get::<i64, _>("updated")?.saturating_mul(scale),
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        db.close().await?;
        Ok::<_, sqlx::Error>(items)
    };
    match tokio::time::timeout(Duration::from_secs(4), operation).await {
        Ok(Ok(items)) => (items, "ok".into()),
        Ok(Err(sqlx::Error::Database(error))) if error.message().contains("no such") => {
            (vec![], "unsupported_schema".into())
        }
        _ => (vec![], "read_error".into()),
    }
}

fn pi_project_dir(workspace: &str) -> String {
    let without_root = workspace.strip_prefix(['/', '\\']).unwrap_or(workspace);
    format!("--{}--", without_root.replace(['/', '\\', ':'], "-"))
}

fn read_pi(root: &Path, workspace: &str, after: Option<&(i64, String)>) -> (Vec<NativeSessionItem>, String) {
    use std::io::{Read, Seek, SeekFrom};
    const MAX_FILES: usize = 2000;
    const HEAD_BYTES: u64 = 4096;
    const TAIL_BYTES: u64 = 32768;
    let directory = root.join(pi_project_dir(workspace));
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (vec![], "missing".into()),
        Err(_) => return (vec![], "read_error".into()),
    };
    let mut candidates = Vec::new();
    let mut partial = false;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_FILES {
            partial = true;
            break;
        }
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        if entry.path().extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        // Do not follow symlinks to arbitrary files or read special devices.
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            partial = true;
            continue;
        };
        let updated_at = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let Ok(mut file) = std::fs::File::open(entry.path()) else {
            partial = true;
            continue;
        };
        let mut bytes = Vec::new();
        if Read::by_ref(&mut file)
            .take(HEAD_BYTES)
            .read_to_end(&mut bytes)
            .is_err()
        {
            partial = true;
            continue;
        }
        let Some(first_line) = bytes.split(|b| *b == b'\n').next() else {
            continue;
        };
        let Ok(header) = serde_json::from_slice::<serde_json::Value>(first_line) else {
            partial = true;
            continue;
        };
        if header["type"] != "session" || header["cwd"].as_str() != Some(workspace) {
            continue;
        }
        let Some(id) = header["id"].as_str().filter(|id| !id.is_empty() && id.len() <= 256) else {
            partial = true;
            continue;
        };
        if after.is_some_and(|(time, key)| updated_at > *time || (updated_at == *time && id >= key.as_str())) {
            continue;
        }
        candidates.push((
            NativeSessionItem {
                id: id.to_owned(),
                title: String::new(),
                workspace: workspace.to_owned(),
                updated_at,
            },
            entry.path(),
        ));
    }
    candidates.sort_by(|a, b| b.0.updated_at.cmp(&a.0.updated_at).then_with(|| b.0.id.cmp(&a.0.id)));
    candidates.truncate(PAGE_SIZE + 1);
    // Read only a bounded tail of the visible page to retrieve explicit session names.
    for (item, path) in &mut candidates {
        let Ok(mut file) = std::fs::File::open(path) else {
            partial = true;
            continue;
        };
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let start = size.saturating_sub(TAIL_BYTES);
        if file.seek(SeekFrom::Start(start)).is_err() {
            partial = true;
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(TAIL_BYTES).read_to_end(&mut bytes).is_err() {
            partial = true;
            continue;
        }
        for line in bytes.split(|b| *b == b'\n').rev() {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if value["type"] == "session_info"
                && let Some(name) = value["name"].as_str()
            {
                item.title = name.chars().take(240).collect();
                break;
            }
        }
    }
    (
        candidates.into_iter().map(|(item, _)| item).collect(),
        if partial { "partial" } else { "ok" }.into(),
    )
}

#[cfg(test)]
#[path = "native_sessions_test.rs"]
mod tests;
