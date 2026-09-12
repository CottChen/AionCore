//! Agent-session operations on ConversationService.
//!
//! These forward to the active AgentInstance (via `self.task(id)`) for
//! config-options/usage/slash-commands/side-question queries, plus workspace
//! browsing that needs the conversations.extra.workspace field.
//!
//! Kept in a separate file from service.rs to avoid pushing that file
//! over 2000 lines.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aionui_ai_agent::{AcpError, AgentError};
use aionui_api_types::{
    ConfigOptionConfirmation, GetConfigOptionsResponse, SetConfigOptionRequest, SetConfigOptionResponse,
    SideQuestionRequest, SideQuestionResponse, SlashCommandItem, WorkspaceBrowseQuery, WorkspaceEntry,
    WorkspaceSearchMatchKind, WorkspaceSearchMode, WorkspaceSearchResponse,
};
use aionui_common::{AgentKillReason, ErrorChain};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::warn;

use crate::ConversationError;
use crate::service::{AssistantRuntimePreferenceUpdate, ConversationService};

const MAX_DIR_DEPTH: usize = 10;
const MAX_WORKSPACE_SEARCH_RESULTS: usize = 200;
const MAX_WORKSPACE_SEARCH_SCANNED: usize = 5000;
const MAX_WORKSPACE_SEARCH_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_WORKSPACE_SEARCH_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const WORKSPACE_SEARCH_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_CONCURRENT_WORKSPACE_SEARCHES: usize = 2;

static WORKSPACE_SEARCH_LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceSearchCursor {
    index: usize,
    query: String,
    path: String,
    mode: WorkspaceSearchMode,
    respect_gitignore: bool,
}

fn encode_workspace_search_cursor(cursor: WorkspaceSearchCursor) -> String {
    let json = serde_json::to_vec(&cursor).expect("workspace search cursor is serializable");
    URL_SAFE_NO_PAD.encode(json)
}

fn decode_workspace_search_cursor(value: &str) -> Result<WorkspaceSearchCursor, ConversationError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ConversationError::BadRequest {
            reason: "cursor is invalid".into(),
        })?;
    serde_json::from_slice(&bytes).map_err(|_| ConversationError::BadRequest {
        reason: "cursor is invalid".into(),
    })
}

fn workspace_content_match_count(path: &Path, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return Some(0);
    };
    if metadata.len() > MAX_WORKSPACE_SEARCH_FILE_BYTES {
        return Some(0);
    }

    let Ok(mut file) = std::fs::File::open(path) else {
        return Some(0);
    };
    let needle_chars = needle.chars().count();
    let mut raw = Vec::with_capacity(WORKSPACE_SEARCH_CHUNK_BYTES + 4);
    let mut buffer = vec![0u8; WORKSPACE_SEARCH_CHUNK_BYTES];
    let mut overlap = String::new();
    let mut matches = 0usize;

    loop {
        let read = match file.read(&mut buffer) {
            Ok(read) => read,
            Err(_) => return Some(0),
        };
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);

        let valid_len = match std::str::from_utf8(&raw) {
            Ok(text) => {
                let count = count_workspace_chunk_matches(text, needle, &overlap);
                matches = matches.saturating_add(count);
                overlap = take_workspace_overlap(&text.to_lowercase(), needle_chars);
                raw.len()
            }
            Err(error) if error.error_len().is_none() => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    let text = std::str::from_utf8(&raw[..valid_len]).expect("valid UTF-8 prefix");
                    let count = count_workspace_chunk_matches(text, needle, &overlap);
                    matches = matches.saturating_add(count);
                    overlap = take_workspace_overlap(&text.to_lowercase(), needle_chars);
                }
                valid_len
            }
            Err(_) => return None,
        };
        if valid_len > 0 {
            raw.drain(..valid_len);
        }
    }

    if !raw.is_empty() {
        let text = std::str::from_utf8(&raw).ok()?;
        matches = matches.saturating_add(count_workspace_chunk_matches(text, needle, &overlap));
    }
    Some(matches)
}

fn take_workspace_overlap(lowercase: &str, needle_chars: usize) -> String {
    lowercase
        .chars()
        .rev()
        .take(needle_chars.saturating_sub(1))
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn count_workspace_chunk_matches(text: &str, needle: &str, overlap: &str) -> usize {
    let lowercase = text.to_lowercase();
    let overlap_bytes = overlap.len();
    let combined = if overlap.is_empty() {
        lowercase
    } else {
        format!("{overlap}{lowercase}")
    };
    combined
        .match_indices(needle)
        .filter(|(start, matched)| start.saturating_add(matched.len()) > overlap_bytes)
        .count()
}

fn workspace_search_entry(
    base: &Path,
    path: &Path,
    is_dir: bool,
    match_kind: WorkspaceSearchMatchKind,
    content_match_count: usize,
) -> Option<WorkspaceEntry> {
    let relative = path.strip_prefix(base).ok()?;
    let name = relative.to_string_lossy().replace('\\', "/");
    if name.is_empty() {
        return None;
    }
    Some(WorkspaceEntry {
        name,
        entry_type: if is_dir { "directory" } else { "file" }.into(),
        match_kind: Some(match_kind),
        content_match_count: (content_match_count > 0).then_some(content_match_count),
    })
}

fn search_workspace_entries_sync(
    base: PathBuf,
    search_root: PathBuf,
    search: String,
    search_mode: WorkspaceSearchMode,
    respect_gitignore: bool,
    cursor: usize,
) -> WorkspaceSearchResponse {
    let needle = search.to_lowercase();
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    let mut scanned_bytes = 0u64;
    let mut absolute_index = 0usize;
    let mut budget_exhausted = false;
    let mut walker_builder = WalkBuilder::new(&search_root);
    walker_builder
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .require_git(false);

    for result in walker_builder.build() {
        let Ok(entry) = result else { continue };
        let path = entry.path();
        if path == search_root {
            continue;
        }
        if absolute_index < cursor {
            absolute_index += 1;
            continue;
        }
        if scanned >= MAX_WORKSPACE_SEARCH_SCANNED || entries.len() >= MAX_WORKSPACE_SEARCH_RESULTS {
            break;
        }
        let Some(file_type) = entry.file_type() else { continue };
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file();
        let relative = path.strip_prefix(&base).ok();
        let name_matches = !matches!(search_mode, WorkspaceSearchMode::Content)
            && relative
                .map(|value| value.to_string_lossy().to_lowercase().contains(&needle))
                .unwrap_or(false);
        let content_match_count = if !matches!(search_mode, WorkspaceSearchMode::Name) && is_file && !name_matches {
            let file_size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if file_size > MAX_WORKSPACE_SEARCH_FILE_BYTES {
                0
            } else if scanned_bytes.saturating_add(file_size) > MAX_WORKSPACE_SEARCH_TOTAL_BYTES {
                budget_exhausted = true;
                break;
            } else {
                scanned_bytes = scanned_bytes.saturating_add(file_size);
                workspace_content_match_count(path, &needle).unwrap_or(0)
            }
        } else {
            0
        };
        absolute_index += 1;
        scanned += 1;
        let content_matches = content_match_count > 0;
        let Some(match_kind) = (if name_matches {
            Some(WorkspaceSearchMatchKind::Name)
        } else if content_matches {
            Some(WorkspaceSearchMatchKind::Content)
        } else {
            None
        }) else {
            continue;
        };
        if let Some(search_entry) = workspace_search_entry(&base, path, is_dir, match_kind, content_match_count) {
            entries.push(search_entry);
        }
    }

    let truncated =
        budget_exhausted || scanned >= MAX_WORKSPACE_SEARCH_SCANNED || entries.len() >= MAX_WORKSPACE_SEARCH_RESULTS;
    WorkspaceSearchResponse {
        entries,
        next_cursor: truncated.then(|| {
            encode_workspace_search_cursor(WorkspaceSearchCursor {
                index: cursor + scanned,
                query: search,
                path: search_root.to_string_lossy().into_owned(),
                mode: search_mode,
                respect_gitignore,
            })
        }),
        scanned,
        truncated,
    }
}

impl ConversationService {
    // ── Config Options ──────────────────────────────────────────────

    pub async fn get_config_options(
        &self,
        conversation_id: &str,
    ) -> Result<GetConfigOptionsResponse, ConversationError> {
        self.task(conversation_id)?
            .get_config_options()
            .await
            .map_err(ConversationError::from)
    }

    pub async fn set_config_option(
        &self,
        conversation_id: &str,
        option_id: &str,
        req: SetConfigOptionRequest,
    ) -> Result<SetConfigOptionResponse, ConversationError> {
        if option_id.trim().is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "option_id must not be empty".into(),
            });
        }
        if req.value.trim().is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "value must not be empty".into(),
            });
        }
        let agent = self.task(conversation_id)?;
        let response = match agent.set_config_option(option_id, &req.value).await {
            Ok(response) => response,
            Err(err @ AgentError::Acp(AcpError::NotConnected)) => {
                warn!(
                    conversation_id,
                    option_id,
                    reason = ?AgentKillReason::AgentErrorRecovery,
                    error = %ErrorChain(&err),
                    "ACP config option failed because protocol is disconnected; evicting task"
                );
                self.task_manager()
                    .kill_and_wait(conversation_id, Some(AgentKillReason::AgentErrorRecovery))
                    .await;
                return Err(ConversationError::from(err));
            }
            Err(err) => return Err(ConversationError::from(err)),
        };

        // Mirror runtime model/mode/thought-level switches into the persisted assistant
        // snapshot + preference so the next conversation seeded from this
        // assistant in `auto` mode reflects the latest pick. We only act on
        // observed confirmations — `command_ack` means the agent merely
        // accepted the request, not that the value is in effect. Persistence
        // failures are logged but do not roll back the
        // user-facing config switch.
        if response.confirmation == ConfigOptionConfirmation::Observed {
            let category = response
                .config_options
                .as_ref()
                .and_then(|options| options.iter().find(|option| option.id == option_id))
                .and_then(|option| option.category.as_deref())
                .unwrap_or(option_id);
            let updates = match category {
                "model" => Some(AssistantRuntimePreferenceUpdate {
                    model: Some(req.value.as_str()),
                    permission: None,
                    thought_level: None,
                }),
                "mode" => Some(AssistantRuntimePreferenceUpdate {
                    model: None,
                    permission: Some(req.value.as_str()),
                    thought_level: None,
                }),
                "thought_level" | "reasoning_effort" => Some(AssistantRuntimePreferenceUpdate {
                    model: None,
                    permission: None,
                    thought_level: Some(req.value.as_str()),
                }),
                _ => None,
            };
            if let Some(updates) = updates {
                if let Err(err) = self.persist_runtime_assistant_snapshot(conversation_id, updates).await {
                    warn!(
                        conversation_id,
                        option_id,
                        error = %ErrorChain(&err),
                        "Failed to persist runtime assistant snapshot after set_config_option",
                    );
                }
                if let Err(err) = self
                    .persist_runtime_assistant_preferences(conversation_id, updates)
                    .await
                {
                    warn!(
                        conversation_id,
                        option_id,
                        error = %ErrorChain(&err),
                        "Failed to persist runtime assistant preferences after set_config_option",
                    );
                }
            }
        }

        Ok(response)
    }

    // ── Usage / Slash commands ──────────────────────────────────────

    pub async fn get_usage(&self, conversation_id: &str) -> Result<Option<serde_json::Value>, ConversationError> {
        self.task(conversation_id)?
            .get_usage()
            .await
            .map_err(ConversationError::from)
    }

    pub async fn get_slash_commands(&self, conversation_id: &str) -> Result<Vec<SlashCommandItem>, ConversationError> {
        self.task(conversation_id)?
            .get_slash_commands()
            .await
            .map_err(ConversationError::from)
    }

    // ── Side question ───────────────────────────────────────────────

    pub async fn handle_side_question(
        &self,
        conversation_id: &str,
        req: SideQuestionRequest,
    ) -> Result<SideQuestionResponse, ConversationError> {
        // `AgentInstance::handle_side_question` already validates that the
        // question is non-empty; no need to duplicate the check here.
        self.task(conversation_id)?
            .handle_side_question(req)
            .await
            .map_err(ConversationError::from)
    }

    // ── Workspace browsing ──────────────────────────────────────────

    /// Enumerate entries under `query.path` inside the conversation's
    /// workspace root. Enforces workspace isolation (no traversal outside
    /// the root, with an allowance for symlinked sub-directories) and a
    /// depth cap of [`MAX_DIR_DEPTH`].
    pub async fn browse_workspace(
        &self,
        conversation_id: &str,
        query: WorkspaceBrowseQuery,
    ) -> Result<Vec<WorkspaceEntry>, ConversationError> {
        if query.path.trim().is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "path must not be empty".into(),
            });
        }

        let row = self
            .conversation_repo()
            .get(conversation_id)
            .await
            .map_err(|e| ConversationError::internal(format!("Failed to load conversation: {e}")))?
            .ok_or_else(|| ConversationError::NotFound {
                id: conversation_id.to_owned(),
            })?;

        let extra: serde_json::Value = serde_json::from_str(&row.extra)
            .map_err(|e| ConversationError::internal(format!("Invalid extra JSON: {e}")))?;
        let workspace = extra
            .get("workspace")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_owned();
        if workspace.is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "Conversation has no workspace assigned".into(),
            });
        }

        let relative_path = query.path.trim_start_matches('/');
        let relative_path_obj = std::path::Path::new(relative_path);
        if relative_path_obj
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(ConversationError::BadRequest {
                reason: "Path traversal outside workspace is not allowed".into(),
            });
        }

        // Resolve the browsed path relative to the workspace root
        let base = std::path::Path::new(&workspace);
        let browse_path = if relative_path.is_empty() {
            base.to_path_buf()
        } else {
            base.join(relative_path_obj)
        };

        // Security: reject direct traversal outside the workspace root, but allow
        // symlinked directories mounted inside the workspace (e.g. native skill
        // dirs that point at the builtin skills corpus under data-dir).
        let canonical_base = base
            .canonicalize()
            .map_err(|e| ConversationError::internal(format!("Failed to resolve workspace path: {e}")))?;
        let canonical_browse = browse_path
            .canonicalize()
            .map_err(|_| ConversationError::not_found_reason("Directory not found"))?;
        if !browse_path.starts_with(base) && !canonical_browse.starts_with(&canonical_base) {
            return Err(ConversationError::BadRequest {
                reason: "Path traversal outside workspace is not allowed".into(),
            });
        }

        // Check depth limit
        let depth = relative_path_obj.components().count();
        if depth > MAX_DIR_DEPTH {
            return Err(ConversationError::BadRequest {
                reason: format!("Directory depth exceeds maximum of {MAX_DIR_DEPTH}"),
            });
        }

        let mut entries = Vec::new();
        let mut dir_reader = tokio::fs::read_dir(&canonical_browse)
            .await
            .map_err(|e| ConversationError::internal(format!("Failed to read directory: {e}")))?;

        while let Ok(Some(entry)) = dir_reader.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();

            // Apply search filter if provided
            if let Some(ref search) = query.search
                && !search.is_empty()
                && !name.to_lowercase().contains(&search.to_lowercase())
            {
                continue;
            }

            let entry_path = entry.path();
            let metadata = tokio::fs::metadata(&entry_path)
                .await
                .map_err(|e| ConversationError::internal(format!("Failed to read entry metadata: {e}")))?;

            let entry_type = if metadata.is_dir() { "directory" } else { "file" };

            entries.push(WorkspaceEntry {
                name,
                entry_type: entry_type.into(),
                match_kind: None,
                content_match_count: None,
            });
        }

        // Sort: directories first, then alphabetically
        entries.sort_by(|a, b| {
            let type_cmp = a.entry_type.cmp(&b.entry_type);
            if type_cmp == std::cmp::Ordering::Equal {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            } else {
                type_cmp
            }
        });

        Ok(entries)
    }

    /// Search a workspace recursively with bounded scanning and opaque cursor
    /// continuation. A root search follows gitignore; an explicitly selected
    /// subdirectory does not, so users can inspect ignored files on demand.
    pub async fn search_workspace(
        &self,
        conversation_id: &str,
        query: WorkspaceBrowseQuery,
    ) -> Result<WorkspaceSearchResponse, ConversationError> {
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ConversationError::BadRequest {
                reason: "search must not be empty".into(),
            })?;

        let row = self
            .conversation_repo()
            .get(conversation_id)
            .await
            .map_err(|e| ConversationError::internal(format!("Failed to load conversation: {e}")))?
            .ok_or_else(|| ConversationError::NotFound {
                id: conversation_id.to_owned(),
            })?;
        let extra: serde_json::Value = serde_json::from_str(&row.extra)
            .map_err(|e| ConversationError::internal(format!("Invalid extra JSON: {e}")))?;
        let workspace = extra
            .get("workspace")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_owned();
        if workspace.is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "Conversation has no workspace assigned".into(),
            });
        }

        let relative_path = query.path.trim_start_matches('/');
        let is_project_root = relative_path.is_empty() || relative_path == ".";
        let relative_path_obj = Path::new(relative_path);
        if relative_path_obj
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(ConversationError::BadRequest {
                reason: "Path traversal outside workspace is not allowed".into(),
            });
        }

        let base = Path::new(&workspace);
        let search_root = if is_project_root {
            base.to_path_buf()
        } else {
            base.join(relative_path_obj)
        };
        let canonical_base = base
            .canonicalize()
            .map_err(|e| ConversationError::internal(format!("Failed to resolve workspace path: {e}")))?;
        let canonical_root = search_root
            .canonicalize()
            .map_err(|_| ConversationError::not_found_reason("Directory not found"))?;
        if !search_root.starts_with(base) && !canonical_root.starts_with(&canonical_base) {
            return Err(ConversationError::BadRequest {
                reason: "Path traversal outside workspace is not allowed".into(),
            });
        }

        let search_mode = query.search_mode.unwrap_or(WorkspaceSearchMode::All);
        let respect_gitignore = query.respect_gitignore.unwrap_or(is_project_root);
        let cursor = if let Some(value) = query.cursor.as_deref() {
            let cursor = decode_workspace_search_cursor(value)?;
            if cursor.query != search
                || cursor.path != canonical_root.to_string_lossy()
                || cursor.mode != search_mode
                || cursor.respect_gitignore != respect_gitignore
            {
                return Err(ConversationError::BadRequest {
                    reason: "cursor does not match this search".into(),
                });
            }
            cursor.index
        } else {
            0
        };
        let base_owned = canonical_base.clone();
        let root_owned = canonical_root;
        let search_owned = search.to_owned();
        let permit = WORKSPACE_SEARCH_LIMITER
            .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_WORKSPACE_SEARCHES)))
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ConversationError::internal("Workspace search limiter closed"))?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            search_workspace_entries_sync(
                base_owned,
                root_owned,
                search_owned,
                search_mode,
                respect_gitignore,
                cursor,
            )
        })
        .await
        .map_err(|error| ConversationError::internal(format!("Workspace search task failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_search_cursor_continues_after_result_limit() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        for index in 0..210 {
            std::fs::write(temp.path().join(format!("match-{index:03}.txt")), "needle").expect("write fixture");
        }

        let first = search_workspace_entries_sync(
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            "needle".to_owned(),
            WorkspaceSearchMode::Content,
            false,
            0,
        );
        assert_eq!(first.entries.len(), MAX_WORKSPACE_SEARCH_RESULTS);
        assert!(first.truncated);
        let next_cursor = first.next_cursor.expect("continuation cursor");

        let second = search_workspace_entries_sync(
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            "needle".to_owned(),
            WorkspaceSearchMode::Content,
            false,
            decode_workspace_search_cursor(&next_cursor)
                .expect("opaque cursor")
                .index,
        );
        assert_eq!(second.entries.len(), 10);
        assert!(!second.truncated);

        let first_names: std::collections::HashSet<_> = first.entries.iter().map(|entry| &entry.name).collect();
        assert!(second.entries.iter().all(|entry| !first_names.contains(&entry.name)));
    }

    #[test]
    fn workspace_search_reads_large_text_files() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let file = temp.path().join("large.txt");
        let mut content = "x".repeat(600 * 1024);
        content.push_str(" needle");
        std::fs::write(&file, content).expect("write fixture");

        let response = search_workspace_entries_sync(
            temp.path().to_path_buf(),
            temp.path().to_path_buf(),
            "needle".to_owned(),
            WorkspaceSearchMode::Content,
            false,
            0,
        );
        assert_eq!(response.entries[0].name, "large.txt");
        assert_eq!(response.entries[0].content_match_count, Some(1));
    }
}
