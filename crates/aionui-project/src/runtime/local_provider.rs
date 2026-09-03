//! `LocalFsProvider` — `file:` scheme [`IFsProvider`] over the local disk.
//!
//! Canonical `file:` URIs are turned into filesystem paths via
//! [`crate::canonical::fs_path`]; all IO goes through `tokio::fs`.
//!
//! TODO(stage-1): realpath containment. Lexical containment already lives in
//! [`crate::containment`] (the reference layer). The access-time boundary —
//! realpath the target before IO and reject symlink/alias escapes out of the
//! Folder root — belongs on the command path and is deferred to the WS handler
//! stage. This provider currently performs no realpath containment.

use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ignore::WalkBuilder;
use uuid::Uuid;

use crate::canonical;

use super::error::FsError;
use super::noise::should_hide;
use super::provider::{EntryFact, IFsProvider, Kind};
use super::search::{
    Budget, CancellationToken, IFsSearchProvider, ProviderSearchHit, SearchLimitReason, SearchMatchKind, SearchQuery,
    SearchSink, SearchWalkResult,
};

/// How often, in walked entries, the blocking walk re-checks the cancel token
/// (checking every entry would be needless overhead on a large tree).
const CANCEL_CHECK_STRIDE: usize = 128;
/// Filename-only search never reads a file, so it can safely cover large source
/// trees before offering the user a continuation page.
const MAX_SEARCH_SCANNED_FILES: usize = 100_000;
const MAX_SEARCH_SESSIONS: usize = 4;
const MAX_SEARCH_CATALOG_BYTES: usize = 16 * 1024 * 1024;
const SEARCH_SESSION_TTL: Duration = Duration::from_secs(120);
/// Keep one pathological generated/log file from dominating an interactive
/// content search. The shared page byte budget is enforced separately.
const MAX_SEARCH_FILE_BYTES: u64 = 4 * 1024 * 1024;
const CONTENT_PREVIEW_CHARS: usize = 240;

/// Local-disk provider for the `file:` scheme.
#[derive(Debug, Clone)]
pub(crate) struct LocalFsProvider {
    search_sessions: Arc<Mutex<SearchSessionStore>>,
}

#[derive(Debug)]
struct SearchSessionStore {
    sessions: HashMap<String, SearchSession>,
}

#[derive(Debug)]
struct SearchSession {
    root: PathBuf,
    entries: Vec<SearchEntry>,
    next_index: usize,
    query: String,
    mode: super::search::SearchMode,
    truncated: bool,
    updated_at: Instant,
}

#[derive(Debug, Clone)]
struct SearchEntry {
    relative_path: String,
    is_directory: bool,
}

impl LocalFsProvider {
    pub fn new() -> Self {
        Self {
            search_sessions: Arc::new(Mutex::new(SearchSessionStore {
                sessions: HashMap::new(),
            })),
        }
    }
}

impl Default for LocalFsProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a `file:` URI to a filesystem path, mapping parse failure to
/// [`FsError::Io`] (a malformed URI is a caller/plumbing fault, not a fs state).
fn path_of(uri: &str) -> Result<PathBuf, FsError> {
    canonical::uri_to_path(uri).map_err(|_| FsError::Io {
        uri: uri.to_owned(),
        message: "invalid file uri".to_owned(),
    })
}

/// Map a std IO error against `uri` to the provider error taxonomy.
fn map_io(uri: &str, err: &io::Error) -> FsError {
    match err.kind() {
        io::ErrorKind::NotFound => FsError::NotFound { uri: uri.to_owned() },
        io::ErrorKind::PermissionDenied => FsError::PermissionDenied { uri: uri.to_owned() },
        io::ErrorKind::AlreadyExists => FsError::AlreadyExists { uri: uri.to_owned() },
        io::ErrorKind::NotADirectory => FsError::NotADirectory { uri: uri.to_owned() },
        _ => FsError::Io {
            uri: uri.to_owned(),
            message: err.to_string(),
        },
    }
}

/// Inode of a file's metadata (0 on platforms without a stable inode).
#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(meta)
}
#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// Last-modified time of a file's metadata as epoch milliseconds, or `None` when
/// it cannot be represented.
///
/// Unlike [`inode_of`], this needs no `#[cfg]` split: `Metadata::modified()` is
/// available on all supported targets (macOS / Windows / Linux × x64 / arm64).
/// It still returns `None` on filesystems that do not record a modification time
/// at all, on pre-epoch timestamps, and on values too large for `i64` — every
/// such case degrades that entry to "never reports modified" (see
/// [`EntryFact::mtime_ms`]).
///
/// Precision caveat: some filesystems only record whole seconds, so two writes
/// inside the same second can leave the timestamp unchanged and the second write
/// goes unreported. That is the under-report direction, which this signal
/// deliberately prefers. Should some platform turn out to under-report as a
/// matter of course, the fallback is to compare `len()` alongside mtime — also
/// free from the metadata already in hand, so a field addition covers it without
/// any change to the surrounding design.
fn mtime_ms_of(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

/// Recursively copy a directory tree using an explicit work stack (avoids
/// boxing an async-recursive fn). Symlinks are copied as their link target
/// content via `fs::copy`, matching shallow-copy semantics.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        tokio::fs::create_dir_all(&to).await?;
        let mut rd = tokio::fs::read_dir(&from).await?;
        while let Some(entry) = rd.next_entry().await? {
            let child_from = entry.path();
            let child_to = to.join(entry.file_name());
            if entry.file_type().await?.is_dir() {
                stack.push((child_from, child_to));
            } else {
                tokio::fs::copy(&child_from, &child_to).await?;
            }
        }
    }
    Ok(())
}

/// Build an [`EntryFact`] from a path via `symlink_metadata`. A symlink remains
/// its own kind, while a separate target hint lets clients render directory
/// links as expandable without folding their identity into the real path.
async fn fact_of(uri: &str, path: &Path) -> Result<EntryFact, FsError> {
    let meta = tokio::fs::symlink_metadata(path).await.map_err(|e| map_io(uri, &e))?;
    let ft = meta.file_type();
    let (kind, symlink_target, symlink_target_is_dir) = if ft.is_symlink() {
        let target = tokio::fs::read_link(path)
            .await
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let target_is_dir = tokio::fs::metadata(path).await.ok().map(|m| m.is_dir());
        (Kind::Symlink, target, target_is_dir)
    } else if ft.is_dir() {
        (Kind::Dir, None, None)
    } else {
        (Kind::File, None, None)
    };
    Ok(EntryFact {
        kind,
        inode: inode_of(&meta),
        symlink_target,
        symlink_target_is_dir,
        // Read off the metadata already fetched above — no extra syscall.
        mtime_ms: mtime_ms_of(&meta),
    })
}

#[async_trait]
impl IFsProvider for LocalFsProvider {
    fn scheme(&self) -> &str {
        "file"
    }

    async fn read_dir(&self, uri: &str) -> Result<Vec<(String, EntryFact)>, FsError> {
        let dir = path_of(uri)?;
        let mut rd = tokio::fs::read_dir(&dir).await.map_err(|e| map_io(uri, &e))?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(|e| map_io(uri, &e))? {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hide OS-junk / VCS-internal noise from the tree listing (same gate
            // the search walk and precise-event apply use — keeps all three
            // pipelines consistent).
            if should_hide(&name) {
                continue;
            }
            let child = entry.path();
            let child_uri = canonical::to_file_uri(&child).unwrap_or_else(|_| uri.to_owned());
            let fact = fact_of(&child_uri, &child).await?;
            out.push((name, fact));
        }
        Ok(out)
    }

    async fn stat(&self, uri: &str) -> Result<Option<EntryFact>, FsError> {
        let path = path_of(uri)?;
        match fact_of(uri, &path).await {
            Ok(fact) => Ok(Some(fact)),
            Err(FsError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn read(&self, uri: &str) -> Result<Vec<u8>, FsError> {
        let path = path_of(uri)?;
        tokio::fs::read(&path).await.map_err(|e| map_io(uri, &e))
    }

    async fn write(&self, uri: &str, data: &[u8]) -> Result<(), FsError> {
        let path = path_of(uri)?;
        tokio::fs::write(&path, data).await.map_err(|e| map_io(uri, &e))
    }

    async fn create_file(&self, uri: &str) -> Result<(), FsError> {
        let path = path_of(uri)?;
        // create_new fails with AlreadyExists rather than truncating.
        tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .map(|_| ())
            .map_err(|e| map_io(uri, &e))
    }

    async fn mkdir(&self, uri: &str) -> Result<(), FsError> {
        let path = path_of(uri)?;
        tokio::fs::create_dir(&path).await.map_err(|e| map_io(uri, &e))
    }

    async fn remove(&self, uri: &str, recursive: bool) -> Result<(), FsError> {
        let path = path_of(uri)?;
        let meta = tokio::fs::symlink_metadata(&path).await.map_err(|e| map_io(uri, &e))?;
        let res = if meta.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        };
        res.map_err(|e| map_io(uri, &e))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), FsError> {
        let (src, dst) = (path_of(from)?, path_of(to)?);
        tokio::fs::rename(&src, &dst).await.map_err(|e| map_io(from, &e))
    }

    async fn copy(&self, from: &str, to: &str, recursive: bool) -> Result<(), FsError> {
        let (src, dst) = (path_of(from)?, path_of(to)?);
        let meta = tokio::fs::symlink_metadata(&src).await.map_err(|e| map_io(from, &e))?;
        if meta.is_dir() {
            if !recursive {
                return Err(FsError::Io {
                    uri: from.to_owned(),
                    message: "cannot copy directory without recursive".to_owned(),
                });
            }
            copy_dir_recursive(&src, &dst).await.map_err(|e| map_io(from, &e))
        } else {
            tokio::fs::copy(&src, &dst)
                .await
                .map(|_| ())
                .map_err(|e| map_io(from, &e))
        }
    }
}

#[async_trait]
impl IFsSearchProvider for LocalFsProvider {
    async fn search(
        &self,
        root_uri: &str,
        query: &SearchQuery,
        sink: &Arc<dyn SearchSink>,
        budget: &Budget,
        cancel: &CancellationToken,
        cursor: Option<&str>,
    ) -> Result<SearchWalkResult, FsError> {
        let root = path_of(root_uri)?;
        // Enumeration and content reads are synchronous and CPU/IO-bound; run
        // them off the async worker. A continuation retains only a bounded list
        // of relative paths, never previous file contents or emitted hits.
        let (query, sink, budget, cancel, cursor, sessions) = (
            query.clone(),
            Arc::clone(sink),
            budget.clone(),
            cancel.clone(),
            cursor.map(str::to_owned),
            Arc::clone(&self.search_sessions),
        );
        tokio::task::spawn_blocking(move || search_page(&root, &query, &sink, &budget, &cancel, cursor, sessions))
            .await
            .map_err(|e| FsError::Io {
                uri: root_uri.to_owned(),
                message: format!("search walk task join failed: {e}"),
            })?
    }
}

/// Build or resume one bounded search session, then process a single result
/// page. Subsequent pages reuse `entries` and `next_index`; they do not walk or
/// sort the directory again.
fn search_page(
    root: &Path,
    query: &SearchQuery,
    sink: &Arc<dyn SearchSink>,
    budget: &Budget,
    cancel: &CancellationToken,
    cursor: Option<String>,
    sessions: Arc<Mutex<SearchSessionStore>>,
) -> Result<SearchWalkResult, FsError> {
    let mut session = match cursor.as_deref() {
        Some(token) => take_search_session(&sessions, token, root, query)?,
        None => {
            let (entries, truncated) = collect_search_entries(root, budget, cancel);
            SearchSession {
                root: root.to_path_buf(),
                entries,
                next_index: 0,
                query: query.needle().to_owned(),
                mode: query.mode(),
                truncated,
                updated_at: Instant::now(),
            }
        }
    };
    if session.truncated {
        budget.mark_limit_reached(SearchLimitReason::ScanLimit);
    }

    let outcome = search_catalog(&mut session, query, sink, budget, cancel);
    if cancel.is_cancelled() || outcome.next_after.is_none() {
        return Ok(outcome);
    }

    let token = cursor.unwrap_or_else(|| Uuid::now_v7().to_string());
    session.updated_at = Instant::now();
    store_search_session(&sessions, token.clone(), session);
    Ok(SearchWalkResult {
        next_after: Some(token),
    })
}

/// Enumerate a stable, bounded catalog once. The list contains only relative
/// paths, so continuation memory is proportional to file names rather than
/// file size or result count.
fn collect_search_entries(root: &Path, budget: &Budget, cancel: &CancellationToken) -> (Vec<SearchEntry>, bool) {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        // Hide OS-junk / VCS-internal noise, matching the tree listing. On a
        // directory this also prevents descent, so `.git` internals never leak
        // into search results (the `ignore` crate does not skip `.git` itself).
        .filter_entry(|entry| entry.file_name().to_str().map(|n| !should_hide(n)).unwrap_or(true));
    // A stable path order makes the catalog deterministic while the session is
    // alive. It is paid once on the first page, never on continuation pages.
    builder.sort_by_file_path(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    let walker = builder.build();

    let mut scanned_files = 0usize;
    let mut entries: Vec<SearchEntry> = Vec::new();
    let mut catalog_bytes = 0usize;
    for (seen, entry) in walker.enumerate() {
        if seen.is_multiple_of(CANCEL_CHECK_STRIDE) && cancel.is_cancelled() {
            return (Vec::new(), false);
        }
        let entry = match entry {
            Ok(e) => e,
            // Unreadable entry (permissions, race) — skip, safely handled.
            Err(err) => {
                tracing::debug!(error = %err, "fs search: skipping unreadable entry");
                continue;
            }
        };
        let file_type = entry.file_type();
        let is_directory = file_type.is_some_and(|ft| ft.is_dir())
            || file_type.is_some_and(|ft| ft.is_symlink() && entry.path().is_dir());
        // The root itself has an empty relative path and is not a searchable
        // result. Descendant directories are retained so their names can match.
        if is_directory && entry.path() == root {
            continue;
        }
        if !is_directory && scanned_files >= MAX_SEARCH_SCANNED_FILES {
            return (entries, true);
        }
        if !is_directory {
            scanned_files += 1;
            budget.record_scanned_file();
        }
        // Keep only the forward-slash relative path. It is sufficient to build
        // the native path later and avoids retaining duplicate name strings.
        let relative_path = rel_path(root, entry.path());
        if catalog_bytes.saturating_add(relative_path.len()) > MAX_SEARCH_CATALOG_BYTES {
            return (entries, true);
        }
        catalog_bytes += relative_path.len();
        entries.push(SearchEntry {
            relative_path,
            is_directory,
        });
    }
    (entries, false)
}

fn take_search_session(
    sessions: &Arc<Mutex<SearchSessionStore>>,
    token: &str,
    root: &Path,
    query: &SearchQuery,
) -> Result<SearchSession, FsError> {
    let mut store = sessions.lock().expect("search session store mutex poisoned");
    prune_search_sessions(&mut store);
    let Some(session) = store.sessions.remove(token) else {
        return Err(FsError::Io {
            uri: root.to_string_lossy().into_owned(),
            message: "search continuation expired or is no longer available".to_owned(),
        });
    };
    if session.root != root || session.query != query.needle() || session.mode != query.mode() {
        return Err(FsError::Io {
            uri: root.to_string_lossy().into_owned(),
            message: "search continuation does not match this request".to_owned(),
        });
    }
    Ok(session)
}

fn store_search_session(sessions: &Arc<Mutex<SearchSessionStore>>, token: String, session: SearchSession) {
    let mut store = sessions.lock().expect("search session store mutex poisoned");
    prune_search_sessions(&mut store);
    while store.sessions.len() >= MAX_SEARCH_SESSIONS {
        let Some(oldest) = store
            .sessions
            .iter()
            .min_by_key(|(_, session)| session.updated_at)
            .map(|(token, _)| token.clone())
        else {
            break;
        };
        store.sessions.remove(&oldest);
    }
    store.sessions.insert(token, session);
}

fn prune_search_sessions(store: &mut SearchSessionStore) {
    let now = Instant::now();
    store
        .sessions
        .retain(|_, session| now.duration_since(session.updated_at) <= SEARCH_SESSION_TTL);
}

fn search_catalog(
    session: &mut SearchSession,
    query: &SearchQuery,
    sink: &Arc<dyn SearchSink>,
    budget: &Budget,
    cancel: &CancellationToken,
) -> SearchWalkResult {
    while session.next_index < session.entries.len() {
        if cancel.is_cancelled() {
            return SearchWalkResult::default();
        }
        let entry = &session.entries[session.next_index];
        let relative_path = &entry.relative_path;
        let path = session.root.join(relative_path);
        // Empty query is browse mode and retains the historical files-only
        // result set; directories participate when a directory name is queried.
        if entry.is_directory && query.needle().is_empty() {
            session.next_index += 1;
            continue;
        }
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            session.next_index += 1;
            continue;
        };
        let name_matches = query.matches_name(&name);
        let content_match = if !entry.is_directory && query.searches_content() {
            match search_file_content(&path, query.needle(), budget) {
                ContentSearch::Match(found) => Some(found),
                ContentSearch::NoMatch => None,
                ContentSearch::BudgetExhausted => return incomplete_search_page(),
            }
        } else {
            None
        };
        session.next_index += 1;
        if !name_matches && content_match.is_none() {
            continue;
        }
        if !budget.try_take() {
            session.next_index -= 1;
            return incomplete_search_page();
        }
        let (content_match_count, content_preview) = content_match
            .map(|(count, preview)| (Some(count), preview))
            .unwrap_or((None, None));
        let match_kind = match (name_matches, content_match_count.is_some()) {
            (true, true) => SearchMatchKind::Both,
            (true, false) => SearchMatchKind::Name,
            (false, true) => SearchMatchKind::Content,
            (false, false) => continue,
        };
        sink.emit(ProviderSearchHit {
            relative_path: relative_path.clone(),
            name,
            is_directory: entry.is_directory,
            match_kind,
            content_match_count,
            content_preview,
        });
    }
    SearchWalkResult::default()
}

fn incomplete_search_page() -> SearchWalkResult {
    SearchWalkResult {
        // Marker only; `search_page` replaces it with an opaque session token.
        next_after: Some(String::new()),
    }
}

enum ContentSearch {
    NoMatch,
    Match((usize, Option<String>)),
    BudgetExhausted,
}

fn search_file_content(path: &Path, needle: &str, budget: &Budget) -> ContentSearch {
    let Ok(metadata) = std::fs::metadata(path) else {
        return ContentSearch::NoMatch;
    };
    if !metadata.is_file() || metadata.len() > MAX_SEARCH_FILE_BYTES {
        if metadata.is_file() {
            budget.record_skipped_large_file();
        }
        return ContentSearch::NoMatch;
    }
    if !budget.try_take_content_bytes(metadata.len()) {
        return ContentSearch::BudgetExhausted;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return ContentSearch::NoMatch;
    };
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return ContentSearch::NoMatch;
    }
    let Ok(content) = String::from_utf8(bytes) else {
        return ContentSearch::NoMatch;
    };
    let lowered = content.to_lowercase();
    let count = lowered.matches(needle).count();
    if count == 0 {
        return ContentSearch::NoMatch;
    }
    let preview = content
        .lines()
        .find(|line| line.to_lowercase().contains(needle))
        .map(|line| line.trim().chars().take(CONTENT_PREVIEW_CHARS).collect::<String>())
        .filter(|line| !line.is_empty());
    ContentSearch::Match((count, preview))
}

/// Root-relative path, forward-slash normalized, no leading slash (wire form).
fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|c| match c {
            Component::Normal(seg) => Some(seg.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
#[path = "local_provider_test.rs"]
mod local_provider_test;
