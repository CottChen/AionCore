//! `IFsSearchProvider` — the recursive project-search capability of a filesystem runtime.
//!
//! Kept a separate trait from [`super::provider::IFsProvider`] so the single-level
//! non-recursive data-op contract is not polluted by the recursive/streaming shape
//! of search. A provider walks its own subtree the most efficient way it can
//! (`LocalFsProvider` = in-process `ignore` walk; a future remote provider = one
//! request + a frame stream), emitting each matching entry through a [`SearchSink`].
//! The provider produces pe-relative hits (files and matching directories); batching and pe-id stamping,
//! merging into one `fs/search` stream, and pushing to the wire are the
//! orchestration layer's job (see `monitor::search`).
//!
//! Feature semantics / engine / chat-ref identity: `formal/runtime/search.md`;
//! protocol: `formal/runtime/protocol.md` `fs/search`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::error::FsError;

/// Total file-content bytes one interactive search page may read across all
/// roots. Individual providers may impose a lower per-file ceiling.
pub const DEFAULT_CONTENT_SEARCH_BYTES: u64 = 128 * 1024 * 1024;

/// How the cheap backend filename predicate matches a candidate name against the
/// query. Backend only *bounds* the hit set cheaply; final fuzzy ranking is the
/// frontend's job (see `search.md` "backend filters, frontend ranks").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Case-insensitive substring — `query` appears contiguously in the name.
    Substring,
    /// Case-insensitive subsequence — `query`'s chars appear in order, gaps ok.
    Subsequence,
}

/// Which parts of a file participate in a project search.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    All,
    #[default]
    Name,
    Content,
}

/// Why one file matched the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMatchKind {
    Name,
    Content,
    Both,
}

/// A safety bound that made a search incomplete, or caused eligible files to
/// be skipped. Kept separate from the hit count so clients can explain whether
/// refining the query, continuing, or changing file-size settings will help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchLimitReason {
    ResultLimit,
    ScanLimit,
    ContentByteLimit,
    FileSizeLimit,
}

/// Metadata collected during one search page. It deliberately contains no
/// paths or query text, so it is safe to place on the monitor wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchProgress {
    pub scanned_files: usize,
    pub searched_content_bytes: u64,
    pub skipped_large_files: usize,
}

/// The outcome of searching one root. `next_after` is an opaque provider-owned
/// continuation token when the page stopped early; `None` means the root was
/// exhausted. The monitor forwards it unchanged and never interprets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchWalkResult {
    pub next_after: Option<String>,
}

/// A precompiled, cheap filename predicate derived from the search `query`.
/// Case-insensitive. An empty query matches every file (panel browse mode).
#[derive(Debug, Clone)]
pub struct NameMatcher {
    /// Lowercased query needle; empty = match-all.
    needle: String,
    mode: MatchMode,
}

impl NameMatcher {
    /// Compile a matcher from the raw query and mode.
    pub fn new(query: &str, mode: MatchMode) -> Self {
        Self {
            needle: query.to_lowercase(),
            mode,
        }
    }

    /// Whether `name` matches. Empty query always matches (browse).
    pub fn matches(&self, name: &str) -> bool {
        if self.needle.is_empty() {
            return true;
        }
        let hay = name.to_lowercase();
        match self.mode {
            MatchMode::Substring => hay.contains(&self.needle),
            MatchMode::Subsequence => is_subsequence(&self.needle, &hay),
        }
    }
}

/// Precompiled project-search predicate shared by every root walk.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    needle: String,
    name_matcher: NameMatcher,
    mode: SearchMode,
}

impl SearchQuery {
    pub fn new(query: &str, mode: SearchMode, match_mode: MatchMode) -> Self {
        Self {
            needle: query.to_lowercase(),
            name_matcher: NameMatcher::new(query, match_mode),
            mode,
        }
    }

    pub fn searches_name(&self) -> bool {
        matches!(self.mode, SearchMode::All | SearchMode::Name)
    }

    pub fn searches_content(&self) -> bool {
        !self.needle.is_empty() && matches!(self.mode, SearchMode::All | SearchMode::Content)
    }

    pub fn matches_name(&self, name: &str) -> bool {
        self.searches_name() && self.name_matcher.matches(name)
    }

    pub fn needle(&self) -> &str {
        &self.needle
    }

    pub fn mode(&self) -> SearchMode {
        self.mode
    }
}

/// Whether every char of `needle` appears in `hay` in order (both prelowered).
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut needle_chars = needle.chars().peekable();
    for c in hay.chars() {
        match needle_chars.peek() {
            Some(&n) if n == c => {
                needle_chars.next();
            }
            Some(_) => {}
            None => return true,
        }
    }
    needle_chars.peek().is_none()
}

/// A hit budget shared across all roots of one search: a global cap on how many
/// files may be emitted before the walk stops. Cloning shares the same counter
/// (cheap `Arc` handle) so concurrent per-root walks draw from one pool.
#[derive(Debug, Clone, Default)]
pub struct Budget(Arc<BudgetInner>);

#[derive(Debug, Default)]
struct BudgetInner {
    /// Remaining emit slots; reaching 0 ends the walk.
    remaining: AtomicUsize,
    /// Set once a walk tried to emit while `remaining == 0` — distinguishes
    /// "hit exactly the cap and there were more" from "found fewer than cap".
    hit_cap: AtomicBool,
    remaining_content_bytes: AtomicU64,
    scanned_files: AtomicUsize,
    searched_content_bytes: AtomicU64,
    skipped_large_files: AtomicUsize,
    reasons: Mutex<BTreeSet<SearchLimitReason>>,
}

impl Budget {
    /// A budget allowing at most `limit` total hits across all roots.
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(BudgetInner {
            remaining: AtomicUsize::new(limit),
            hit_cap: AtomicBool::new(false),
            remaining_content_bytes: AtomicU64::new(u64::MAX),
            scanned_files: AtomicUsize::new(0),
            searched_content_bytes: AtomicU64::new(0),
            skipped_large_files: AtomicUsize::new(0),
            reasons: Mutex::new(BTreeSet::new()),
        }))
    }

    /// A hit budget with a shared upper bound on bytes read for content search.
    /// The budget is global across roots, preventing a multi-root query from
    /// multiplying the configured resource use.
    pub fn with_content_byte_limit(limit: usize, content_bytes: u64) -> Self {
        let budget = Self::new(limit);
        budget.0.remaining_content_bytes.store(content_bytes, Ordering::Relaxed);
        budget
    }

    /// Reserve one emit slot. Returns `true` if a slot was taken; `false` when
    /// the budget is exhausted (and records that the cap forced a stop).
    pub fn try_take(&self) -> bool {
        let taken = self
            .0
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| cur.checked_sub(1))
            .is_ok();
        if !taken {
            self.0.hit_cap.store(true, Ordering::Relaxed);
            self.mark_limit_reached(SearchLimitReason::ResultLimit);
        }
        taken
    }

    /// Whether any walk was forced to stop because the cap was reached.
    pub fn limit_reached(&self) -> bool {
        self.0.hit_cap.load(Ordering::Relaxed) || !self.limit_reasons().is_empty()
    }

    /// Record a named safety bound. A single search can hit more than one
    /// reason across different roots, so reasons are accumulated.
    pub fn mark_limit_reached(&self, reason: SearchLimitReason) {
        self.0.hit_cap.store(true, Ordering::Relaxed);
        self.0
            .reasons
            .lock()
            .expect("search budget reasons mutex poisoned")
            .insert(reason);
    }

    /// Reserve bytes before reading one content file. A failed reservation
    /// stops that root page before the file, making the returned cursor safe to
    /// resume from without skipping the candidate.
    pub fn try_take_content_bytes(&self, bytes: u64) -> bool {
        let taken = self
            .0
            .remaining_content_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_ok();
        if taken {
            self.0.searched_content_bytes.fetch_add(bytes, Ordering::Relaxed);
        } else {
            self.mark_limit_reached(SearchLimitReason::ContentByteLimit);
        }
        taken
    }

    pub fn record_scanned_file(&self) {
        self.0.scanned_files.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_skipped_large_file(&self) {
        self.0.skipped_large_files.fetch_add(1, Ordering::Relaxed);
        self.mark_limit_reached(SearchLimitReason::FileSizeLimit);
    }

    pub fn progress(&self) -> SearchProgress {
        SearchProgress {
            scanned_files: self.0.scanned_files.load(Ordering::Relaxed),
            searched_content_bytes: self.0.searched_content_bytes.load(Ordering::Relaxed),
            skipped_large_files: self.0.skipped_large_files.load(Ordering::Relaxed),
        }
    }

    pub fn limit_reasons(&self) -> Vec<SearchLimitReason> {
        self.0
            .reasons
            .lock()
            .expect("search budget reasons mutex poisoned")
            .iter()
            .copied()
            .collect()
    }
}

/// Cooperative cancel signal, cascaded to every per-root walk of a search.
/// Explicit `fs/searchCancel` or a superseding new search flips it; each walk
/// checks it and stops (a future remote provider kills its remote request).
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Hit outlet. The provider calls [`SearchSink::emit`] per matching entry with the
/// root-relative path (forward-slash normalized, no leading slash) and entry name;
/// the orchestration layer stamps `pe_id`, batches, and pushes `fs/searchMatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSearchHit {
    pub relative_path: String,
    pub name: String,
    pub is_directory: bool,
    pub match_kind: SearchMatchKind,
    pub content_match_count: Option<usize>,
    pub content_preview: Option<String>,
}

pub trait SearchSink: Send + Sync {
    /// Emit one matching file within the current root.
    fn emit(&self, hit: ProviderSearchHit);
}

/// Recursive search capability for one provider scheme. Distinct from
/// [`IFsProvider`](super::provider::IFsProvider): recursive + streaming.
#[async_trait]
pub trait IFsSearchProvider: Send + Sync {
    /// Walk `root_uri`'s subtree, emitting each file satisfying `query` through
    /// `sink`, until the subtree is exhausted, `budget` runs
    /// out, or `cancel` fires. `budget` and `cancel` are shared across all roots
    /// of the search; the sink merges every root into one stream.
    async fn search(
        &self,
        root_uri: &str,
        query: &SearchQuery,
        sink: &Arc<dyn SearchSink>,
        budget: &Budget,
        cancel: &CancellationToken,
        cursor: Option<&str>,
    ) -> Result<SearchWalkResult, FsError>;
}

#[cfg(test)]
#[path = "search_test.rs"]
mod search_test;
