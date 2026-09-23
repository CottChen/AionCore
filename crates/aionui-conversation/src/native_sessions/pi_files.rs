//! Bounded, shallow discovery of Pi's standard session directory. Never scan a workspace/home tree.
use aionui_api_types::NativeSessionItem;
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

#[derive(Clone)]
pub(super) struct Entry {
    pub item: NativeSessionItem,
    pub path: PathBuf,
}
pub(super) struct Snapshot {
    pub id: u64,
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub partial: bool,
    at: Instant,
}
static CACHE: LazyLock<Mutex<Vec<Arc<Snapshot>>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TTL: Duration = Duration::from_secs(300);

pub(super) fn snapshot(root: &Path, requested: Option<u64>, refresh: bool) -> Result<Arc<Snapshot>, String> {
    {
        let mut cache = CACHE.lock().map_err(|_| "read_error")?;
        cache.retain(|s| s.at.elapsed() < TTL);
        if let Some(found) = cache
            .iter()
            .rev()
            .find(|s| s.root == root && requested.is_none_or(|id| id == s.id))
            && (requested.is_some() || !refresh)
        {
            return Ok(found.clone());
        }
        if requested.is_some() {
            return Err("stale_cursor".into());
        }
    }
    let started = Instant::now();
    let projects = std::fs::read_dir(root).map_err(io_status)?;
    let mut entries = Vec::new();
    let mut visited = 0;
    let mut partial = false;
    let mut bytes_read = 0;
    'scan: for (i, project) in projects.enumerate() {
        if i >= 2000 || started.elapsed() > Duration::from_secs(2) {
            partial = true;
            break;
        }
        let Ok(project) = project else {
            partial = true;
            continue;
        };
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project.path()) else {
            partial = true;
            continue;
        };
        for entry in files {
            visited += 1;
            if visited > 5000 || bytes_read >= 16 * 1024 * 1024 || started.elapsed() > Duration::from_secs(2) {
                partial = true;
                break 'scan;
            }
            let Ok(entry) = entry else {
                partial = true;
                continue;
            };
            if entry.path().extension().is_none_or(|e| e != "jsonl") || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            let Ok(mut file) = File::open(entry.path()) else {
                partial = true;
                continue;
            };
            let mut head = Vec::new();
            if Read::by_ref(&mut file).take(4096).read_to_end(&mut head).is_err() {
                partial = true;
                continue;
            }
            bytes_read += head.len();
            let Some(line) = head.split(|b| *b == b'\n').next() else {
                continue;
            };
            let Ok(header) = serde_json::from_slice::<Value>(line) else {
                partial = true;
                continue;
            };
            if header["type"] != "session" {
                partial = true;
                continue;
            }
            let Some(id) = header["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 256) else {
                partial = true;
                continue;
            };
            let Some(cwd) = header["cwd"].as_str().filter(|s| s.len() <= 4096) else {
                partial = true;
                continue;
            };
            let updated = file
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            entries.push(Entry {
                item: NativeSessionItem {
                    id: id.into(),
                    title: String::new(),
                    workspace: cwd.into(),
                    updated_at: updated,
                    created_at: header["timestamp"].as_str().and_then(timestamp),
                    model: None,
                },
                path: entry.path(),
            });
        }
    }
    entries.sort_by(|a, b| {
        b.item
            .updated_at
            .cmp(&a.item.updated_at)
            .then_with(|| b.item.id.cmp(&a.item.id))
    });
    let result = Arc::new(Snapshot {
        id: SEQUENCE.fetch_add(1, Ordering::Relaxed),
        root: root.into(),
        entries,
        partial,
        at: Instant::now(),
    });
    let mut cache = CACHE.lock().map_err(|_| "read_error")?;
    if cache.len() >= 2 {
        cache.remove(0);
    }
    cache.push(result.clone());
    Ok(result)
}

pub(super) fn find(root: &Path, id: &str) -> Result<Entry, String> {
    for refresh in [false, true] {
        let snapshot = snapshot(root, None, refresh)?;
        if let Some(entry) = snapshot.entries.iter().find(|e| e.item.id == id) {
            return Ok(entry.clone());
        }
        if refresh && snapshot.partial {
            return Err("partial".into());
        }
    }
    Err("missing".into())
}

/// Only enrich the visible page (at most 20 items), never every transcript in the catalog.
pub(super) fn enrich(root: &Path, entry: &Entry) -> Result<NativeSessionItem, String> {
    let mut item = entry.item.clone();
    let mut file = super::transcript::open_file(root, &entry.path)?;
    let mut head = Vec::new();
    Read::by_ref(&mut file)
        .take(32768)
        .read_to_end(&mut head)
        .map_err(io_status)?;
    for line in head.split(|b| *b == b'\n') {
        if let Ok(v) = serde_json::from_slice::<Value>(line)
            && v["type"] == "message"
            && v["message"]["role"] == "user"
        {
            item.title = super::transcript::text(&v["message"]["content"], 240).0;
            break;
        }
    }
    let length = file.metadata().map_err(io_status)?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(65536)))
        .map_err(io_status)?;
    let mut tail = Vec::new();
    file.take(65536).read_to_end(&mut tail).map_err(io_status)?;
    let mut named = false;
    for line in tail.split(|b| *b == b'\n').rev() {
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            if !named
                && v["type"] == "session_info"
                && let Some(name) = v["name"].as_str()
            {
                item.title = name.chars().take(240).collect();
                named = true;
            }
            if item.model.is_none() {
                item.model = if v["type"] == "model_change" {
                    v["modelId"].as_str()
                } else {
                    v["message"]["model"].as_str()
                }
                .map(|s| s.chars().take(200).collect());
            }
        }
    }
    Ok(item)
}

pub(super) fn timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp_millis())
}
pub(super) fn io_status(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "missing"
    } else {
        "read_error"
    }
    .into()
}
