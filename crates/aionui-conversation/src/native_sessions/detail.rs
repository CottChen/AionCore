use super::{CATALOG_READS, DETAIL_PAGE_SIZE, catalog, native_root, pi_files, transcript};
use aionui_api_types::{NativeSessionBackend, NativeSessionDetailResponse, NativeSessionItem, NativeSessionMessage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, Row, SqliteConnection};
use std::{path::Path, time::Duration};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartCursor {
    backend: NativeSessionBackend,
    session: String,
    time: i64,
    id: String,
}

pub(crate) async fn get(
    backend: NativeSessionBackend,
    id: &str,
    cursor: Option<&str>,
    requested_limit: Option<u32>,
) -> Result<NativeSessionDetailResponse, String> {
    if id.is_empty()
        || id.len() > 256
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || cursor.is_some_and(|s| s.len() > 8192)
        || requested_limit.is_some_and(|v| v == 0 || v > DETAIL_PAGE_SIZE as u32)
    {
        return Err("bad_request".into());
    }
    let root = native_root(backend).ok_or("missing")?;
    let limit = requested_limit.map_or(DETAIL_PAGE_SIZE, |v| v as usize);
    let permit = tokio::time::timeout(Duration::from_secs(1), CATALOG_READS.clone().acquire_owned())
        .await
        .map_err(|_| "read_error")?
        .map_err(|_| "read_error")?;
    if backend == NativeSessionBackend::Pi {
        let id = id.to_owned();
        let cursor = cursor.map(str::to_owned);
        return tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let entry = pi_files::find(&root, &id)?;
            let session = pi_files::enrich(&root, &entry)?;
            let (messages, next_cursor, partial) =
                transcript::page(&root, &entry.path, backend, &id, cursor.as_deref(), limit)?;
            Ok(result(backend, session, messages, next_cursor, partial))
        })
        .await
        .map_err(|_| "read_error")?;
    }
    let (session, path, parts) =
        tokio::time::timeout(Duration::from_secs(4), metadata(&root, backend, id, cursor, limit))
            .await
            .map_err(|_| "read_error")??;
    if let Some(path) = path {
        let cursor = cursor.map(str::to_owned);
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let (messages, next_cursor, partial) =
                transcript::page(&root, Path::new(&path), backend, &id, cursor.as_deref(), limit)?;
            Ok(result(backend, session, messages, next_cursor, partial))
        })
        .await
        .map_err(|_| "read_error")?
    } else {
        drop(permit);
        let (messages, next_cursor, partial) = parts.ok_or("read_error")?;
        Ok(result(backend, session, messages, next_cursor, partial))
    }
}

type Parts = (Vec<NativeSessionMessage>, Option<String>, bool);
async fn metadata(
    root: &Path,
    backend: NativeSessionBackend,
    id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<(NativeSessionItem, Option<String>, Option<Parts>), String> {
    let mut db = catalog::open(root, backend).await?;
    let (table, _, _, _, scale) = catalog::schema(backend);
    let columns = catalog::columns(&mut db, table).await?;
    let selection = catalog::selection(backend, &columns);
    let extra = if backend == NativeSessionBackend::Codex {
        ", substr(rollout_path,1,4096) AS path"
    } else {
        ""
    };
    let row = sqlx::query(&format!("SELECT {selection}{extra} FROM {table} WHERE id = ?"))
        .bind(id)
        .fetch_optional(&mut db)
        .await
        .map_err(catalog::db_error)?
        .ok_or("missing")?;
    let item = catalog::item(&row, scale)?;
    let path = if backend == NativeSessionBackend::Codex {
        Some(row.try_get::<String, _>("path").map_err(catalog::db_error)?)
    } else {
        None
    };
    let parts = if backend == NativeSessionBackend::Opencode {
        Some(read_parts(&mut db, id, cursor, limit).await?)
    } else {
        None
    };
    db.close().await.map_err(catalog::db_error)?;
    Ok((item, path, parts))
}

async fn read_parts(db: &mut SqliteConnection, id: &str, cursor: Option<&str>, limit: usize) -> Result<Parts, String> {
    let position: Option<PartCursor> = cursor.map(catalog::decode).transpose()?;
    if position
        .as_ref()
        .is_some_and(|p| p.backend != NativeSessionBackend::Opencode || p.session != id)
    {
        return Err("bad_request".into());
    }
    let rows = sqlx::query("SELECT p.id, substr(p.data,1,32768) AS data, length(p.data) AS size, p.time_created AS time, json_extract(CASE WHEN length(m.data) <= 65536 THEN CASE WHEN json_valid(m.data) THEN m.data ELSE '{}' END ELSE '{}' END, '$.role') AS role FROM part p JOIN message m ON m.id = p.message_id AND m.session_id = p.session_id WHERE p.session_id = ? AND (? = 0 OR p.time_created < ? OR (p.time_created = ? AND p.id < ?)) ORDER BY p.time_created DESC, p.id DESC LIMIT ?")
        .bind(id).bind(i32::from(position.is_some())).bind(position.as_ref().map_or(0, |p| p.time)).bind(position.as_ref().map_or(0, |p| p.time)).bind(position.as_ref().map_or("", |p| p.id.as_str())).bind((limit + 1) as i64).fetch_all(&mut *db).await.map_err(catalog::db_error)?;
    if rows.is_empty() && position.is_none() {
        let has_new = sqlx::query("SELECT 1 FROM session_message WHERE session_id = ? LIMIT 1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await;
        if matches!(has_new, Ok(Some(_))) {
            return Err("unsupported_schema".into());
        }
    }
    let mut messages = Vec::new();
    let mut partial = false;
    for row in rows.iter().take(limit) {
        let data: String = row.try_get("data").map_err(catalog::db_error)?;
        let size: i64 = row.try_get("size").map_err(catalog::db_error)?;
        let value = if size > 32768 {
            None
        } else {
            serde_json::from_str::<Value>(&data).ok()
        };
        let (kind, body) = if let Some(v) = &value {
            let kind = v["type"].as_str().unwrap_or("event");
            if matches!(kind, "step-start" | "step-finish") {
                continue;
            }
            let body = if kind == "tool" {
                Value::String(format!(
                    "{}\n{}\n{}",
                    v["tool"].as_str().unwrap_or(""),
                    v["state"]["input"],
                    v["state"]["output"].as_str().unwrap_or("")
                ))
            } else {
                v.clone()
            };
            (kind.to_owned(), body)
        } else {
            partial = true;
            (if size > 32768 { "oversized" } else { "malformed" }.into(), Value::Null)
        };
        let (text, truncated) = transcript::text(&body, transcript::TEXT_CHARS);
        messages.push(NativeSessionMessage {
            id: row.try_get("id").map_err(catalog::db_error)?,
            role: row
                .try_get::<Option<String>, _>("role")
                .map_err(catalog::db_error)?
                .unwrap_or("unknown".into()),
            kind,
            text,
            timestamp: Some(row.try_get("time").map_err(catalog::db_error)?),
            truncated: truncated || value.is_none(),
        });
    }
    let next = if rows.len() > limit {
        let row = &rows[limit - 1];
        Some(catalog::encode(&PartCursor {
            backend: NativeSessionBackend::Opencode,
            session: id.into(),
            time: row.try_get("time").map_err(catalog::db_error)?,
            id: row.try_get("id").map_err(catalog::db_error)?,
        }))
    } else {
        None
    };
    messages.reverse();
    Ok((messages, next, partial))
}

fn result(
    backend: NativeSessionBackend,
    session: NativeSessionItem,
    messages: Vec<NativeSessionMessage>,
    next_cursor: Option<String>,
    partial: bool,
) -> NativeSessionDetailResponse {
    NativeSessionDetailResponse {
        backend,
        session,
        messages,
        next_cursor,
        total_messages: None,
        status: if partial { "partial" } else { "ok" }.into(),
    }
}

#[cfg(test)]
#[path = "detail_tests.rs"]
mod tests;
