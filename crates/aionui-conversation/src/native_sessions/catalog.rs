use super::{CATALOG_READS, PAGE_SIZE, native_root, pi_files};
use aionui_api_types::{NativeSessionBackend, NativeSessionCatalogResponse, NativeSessionItem};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::{
    Connection, Row, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteRow},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Deserialize, Default)]
struct Position {
    time: i64,
    id: String,
    snapshot: Option<u64>,
    index: usize,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    backend: NativeSessionBackend,
    search: String,
    position: Position,
}

pub(crate) async fn list(
    backend: NativeSessionBackend,
    cursor: Option<&str>,
    search: Option<&str>,
) -> Result<NativeSessionCatalogResponse, String> {
    let search = search.unwrap_or("").trim();
    if search.len() > 256 {
        return Err("bad_request".into());
    }
    let position = if let Some(raw) = cursor {
        let c: Cursor = decode(raw)?;
        if c.backend != backend || c.search != search {
            return Err("bad_request".into());
        }
        Some(c.position)
    } else {
        None
    };
    let permit = tokio::time::timeout(Duration::from_secs(1), CATALOG_READS.clone().acquire_owned())
        .await
        .map_err(|_| "read_error")?
        .map_err(|_| "read_error")?;
    let root = native_root(backend).ok_or("missing")?;
    let page = if backend == NativeSessionBackend::Pi {
        let search = search.to_owned();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            list_pi(&root, position.as_ref(), &search)
        })
        .await
        .map_err(|_| "read_error")?
    } else {
        let result = tokio::time::timeout(
            Duration::from_secs(4),
            list_sqlite(&root, backend, position.as_ref(), search),
        )
        .await
        .map_err(|_| "read_error")?;
        drop(permit);
        result
    };
    match page {
        Ok((items, next, partial)) => Ok(NativeSessionCatalogResponse {
            backend,
            items,
            next_cursor: next.map(|position| {
                encode(&Cursor {
                    backend,
                    search: search.into(),
                    position,
                })
            }),
            status: if partial { "partial" } else { "ok" }.into(),
        }),
        Err(status) if matches!(status.as_str(), "missing" | "unsupported_schema" | "read_error") => {
            Ok(NativeSessionCatalogResponse {
                backend,
                items: vec![],
                next_cursor: None,
                status,
            })
        }
        Err(error) => Err(error),
    }
}

type Page = Result<(Vec<NativeSessionItem>, Option<Position>, bool), String>;
fn list_pi(root: &Path, position: Option<&Position>, search: &str) -> Page {
    let snapshot = pi_files::snapshot(root, position.and_then(|p| p.snapshot), position.is_none())?;
    let start = position.map_or(0, |p| p.index);
    if start > snapshot.entries.len() {
        return Err("bad_request".into());
    }
    let query = search.to_lowercase();
    let mut items = Vec::new();
    let mut partial = snapshot.partial;
    for (index, entry) in snapshot.entries.iter().enumerate().skip(start) {
        if !query.is_empty()
            && !entry.item.id.to_lowercase().contains(&query)
            && !entry.item.workspace.to_lowercase().contains(&query)
        {
            continue;
        }
        if items.len() == PAGE_SIZE {
            return Ok((
                items,
                Some(Position {
                    snapshot: Some(snapshot.id),
                    index,
                    ..Position::default()
                }),
                partial,
            ));
        }
        match pi_files::enrich(root, entry) {
            Ok(item) => items.push(item),
            Err(_) => {
                partial = true;
                items.push(entry.item.clone());
            }
        }
    }
    Ok((items, None, partial))
}

async fn list_sqlite(root: &Path, backend: NativeSessionBackend, position: Option<&Position>, search: &str) -> Page {
    let mut db = open(root, backend).await?;
    let (table, cwd, updated, _, scale) = schema(backend);
    let columns = columns(&mut db, table).await?;
    let selection = selection(backend, &columns);
    let sql = format!(
        "SELECT {selection} FROM {table} WHERE (? = 0 OR {updated} < ? OR ({updated} = ? AND id < ?)) AND (? = '' OR instr(lower(id), lower(?)) > 0 OR instr(lower({cwd}), lower(?)) > 0) ORDER BY {updated} DESC, id DESC LIMIT ?"
    );
    let time = position.map_or(0, |p| p.time / scale);
    let rows = sqlx::query(&sql)
        .bind(i32::from(position.is_some()))
        .bind(time)
        .bind(time)
        .bind(position.map_or("", |p| p.id.as_str()))
        .bind(search)
        .bind(search)
        .bind(search)
        .bind((PAGE_SIZE + 1) as i64)
        .fetch_all(&mut db)
        .await
        .map_err(db_error)?;
    let mut items = rows.iter().map(|r| item(r, scale)).collect::<Result<Vec<_>, _>>()?;
    let next = if items.len() > PAGE_SIZE {
        items.truncate(PAGE_SIZE);
        items.last().map(|i| Position {
            time: i.updated_at,
            id: i.id.clone(),
            ..Position::default()
        })
    } else {
        None
    };
    db.close().await.map_err(db_error)?;
    Ok((items, next, false))
}

pub(super) fn encode<T: Serialize>(value: &T) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).expect("serializable cursor"))
}
pub(super) fn decode<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T, String> {
    if raw.len() > 8192 {
        return Err("bad_request".into());
    }
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw).map_err(|_| "bad_request")?).map_err(|_| "bad_request".into())
}

pub(super) fn schema(backend: NativeSessionBackend) -> (&'static str, &'static str, &'static str, &'static str, i64) {
    if backend == NativeSessionBackend::Codex {
        ("threads", "cwd", "updated_at", "created_at", 1000)
    } else {
        ("session", "directory", "time_updated", "time_created", 1)
    }
}
pub(super) fn selection(backend: NativeSessionBackend, columns: &HashSet<String>) -> String {
    let (_, cwd, updated, created, _) = schema(backend);
    let created = if columns.contains(created) { created } else { "NULL" };
    let model = if columns.contains("model") {
        "substr(model,1,200)"
    } else {
        "NULL"
    };
    format!(
        "substr(id,1,256) AS id, substr(title,1,240) AS title, substr({cwd},1,4096) AS workspace, {updated} AS updated, {created} AS created, {model} AS model"
    )
}
pub(super) fn item(row: &SqliteRow, scale: i64) -> Result<NativeSessionItem, String> {
    let model: Option<String> = row.try_get("model").map_err(db_error)?;
    let model = model.map(|s| {
        serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| v["id"].as_str().map(str::to_owned))
            .unwrap_or(s)
    });
    Ok(NativeSessionItem {
        id: row.try_get("id").map_err(db_error)?,
        title: row.try_get("title").map_err(db_error)?,
        workspace: row.try_get("workspace").map_err(db_error)?,
        updated_at: row
            .try_get::<i64, _>("updated")
            .map_err(db_error)?
            .saturating_mul(scale),
        created_at: row
            .try_get::<Option<i64>, _>("created")
            .map_err(db_error)?
            .map(|v| v.saturating_mul(scale)),
        model,
    })
}
pub(super) async fn columns(db: &mut SqliteConnection, table: &str) -> Result<HashSet<String>, String> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(db)
        .await
        .map_err(db_error)?;
    if rows.is_empty() {
        return Err("unsupported_schema".into());
    }
    rows.iter()
        .map(|r| r.try_get::<String, _>("name").map_err(db_error))
        .collect()
}
pub(super) async fn open(root: &Path, backend: NativeSessionBackend) -> Result<SqliteConnection, String> {
    let path = database(root, backend).await?;
    let metadata = tokio::fs::metadata(&path).await.map_err(pi_files::io_status)?;
    if !metadata.is_file() {
        return Err("read_error".into());
    }
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(1)),
    )
    .await
    .map_err(db_error)
}
async fn database(root: &Path, backend: NativeSessionBackend) -> Result<PathBuf, String> {
    if backend == NativeSessionBackend::Opencode {
        return Ok(root.join("opencode.db"));
    }
    let mut entries = tokio::fs::read_dir(root).await.map_err(pi_files::io_status)?;
    let mut found: Option<(u32, PathBuf)> = None;
    for _ in 0..512 {
        let Some(entry) = entries.next_entry().await.map_err(pi_files::io_status)? else {
            return found.map(|(_, p)| p).ok_or("missing".into());
        };
        let name = entry.file_name();
        if let Some(v) = name
            .to_str()
            .and_then(|n| n.strip_prefix("state_"))
            .and_then(|n| n.strip_suffix(".sqlite"))
            .and_then(|n| n.parse::<u32>().ok())
            && found.as_ref().is_none_or(|(old, _)| v > *old)
        {
            found = Some((v, entry.path()));
        }
    }
    Err("read_error".into())
}
pub(super) fn db_error(error: sqlx::Error) -> String {
    if matches!(&error, sqlx::Error::Database(e) if e.message().contains("no such")) {
        "unsupported_schema"
    } else {
        "read_error"
    }
    .into()
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
