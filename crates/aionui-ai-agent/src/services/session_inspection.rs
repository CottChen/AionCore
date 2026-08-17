use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aionui_api_types::{
    AgentSessionBackend, AgentSessionChildTask, AgentSessionItem, AgentSessionItemKind, AgentSessionScope,
    AgentSessionSnapshot, AgentSessionSummary, AgentSessionTurn,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::error::AgentError;

const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 500;
const MAX_ITEMS: usize = 4_000;
const MAX_VALUE_CHARS: usize = 100_000;

#[derive(Debug, Clone)]
pub struct AgentSessionInspectionService {
    codex_home: PathBuf,
    opencode_data_dir: PathBuf,
    pi_agent_dir: PathBuf,
}

impl AgentSessionInspectionService {
    pub fn new() -> Arc<Self> {
        let home = dirs::home_dir().unwrap_or_default();
        let codex_home = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let pi_agent_dir = env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi").join("agent"));
        let opencode_data_dir = env::var_os("OPENCODE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_DATA_HOME").map(|root| PathBuf::from(root).join("opencode")))
            .unwrap_or_else(|| {
                let xdg_dir = home.join(".local").join("share").join("opencode");
                if xdg_dir.exists() {
                    xdg_dir
                } else {
                    dirs::data_local_dir()
                        .unwrap_or_else(|| home.join(".local").join("share"))
                        .join("opencode")
                }
            });
        Arc::new(Self {
            codex_home,
            opencode_data_dir,
            pi_agent_dir,
        })
    }

    #[cfg(test)]
    fn with_roots(codex_home: PathBuf, opencode_data_dir: PathBuf, pi_agent_dir: PathBuf) -> Self {
        Self {
            codex_home,
            opencode_data_dir,
            pi_agent_dir,
        }
    }

    pub async fn list(
        &self,
        backend: AgentSessionBackend,
        scope: AgentSessionScope,
        limit: Option<usize>,
    ) -> Result<Vec<AgentSessionSummary>, AgentError> {
        let service = self.clone();
        let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
        tokio::task::spawn_blocking(move || match backend {
            AgentSessionBackend::Codex => service.list_codex(scope, limit),
            AgentSessionBackend::Opencode => service.list_opencode(scope, limit),
            AgentSessionBackend::Pi => service.list_pi(scope, limit),
        })
        .await
        .map_err(|error| AgentError::internal(format!("CLI session reader stopped unexpectedly: {error}")))?
    }

    pub async fn inspect(&self, backend: AgentSessionBackend, id: String) -> Result<AgentSessionSnapshot, AgentError> {
        validate_session_id(&id)?;
        let service = self.clone();
        tokio::task::spawn_blocking(move || match backend {
            AgentSessionBackend::Codex => service.inspect_codex(&id),
            AgentSessionBackend::Opencode => service.inspect_opencode(&id),
            AgentSessionBackend::Pi => service.inspect_pi(&id),
        })
        .await
        .map_err(|error| AgentError::internal(format!("CLI session reader stopped unexpectedly: {error}")))?
    }

    fn list_codex(&self, scope: AgentSessionScope, limit: usize) -> Result<Vec<AgentSessionSummary>, AgentError> {
        let Some(db_path) = find_codex_state_db(&self.codex_home)? else {
            return Ok(Vec::new());
        };
        let connection = open_read_only(&db_path)?;
        let scope_clause = match scope {
            AgentSessionScope::All => "",
            AgentSessionScope::Main => "WHERE e.parent_thread_id IS NULL",
            AgentSessionScope::Child => "WHERE e.parent_thread_id IS NOT NULL",
        };
        let sql = format!(
            "SELECT t.id, t.title, t.cwd, t.model, t.source, t.archived, \
                        COALESCE(t.created_at_ms, t.created_at * 1000), \
                        COALESCE(t.updated_at_ms, t.updated_at * 1000), e.parent_thread_id \
                 FROM threads t LEFT JOIN thread_spawn_edges e ON e.child_thread_id = t.id \
                 {scope_clause} \
                 ORDER BY COALESCE(NULLIF(t.recency_at_ms, 0), t.updated_at_ms, t.updated_at * 1000) DESC \
                 LIMIT ?1"
        );
        let mut statement = connection.prepare(&sql).map_err(sql_error)?;
        let rows = statement
            .query_map(params![limit as i64], |row| {
                Ok(AgentSessionSummary {
                    id: row.get(0)?,
                    backend: AgentSessionBackend::Codex,
                    title: non_empty(row.get(1)?),
                    cwd: non_empty(row.get(2)?),
                    model: row.get(3)?,
                    source: non_empty(row.get(4)?),
                    status: Some(
                        if row.get::<_, i64>(5)? == 0 {
                            "active"
                        } else {
                            "archived"
                        }
                        .to_owned(),
                    ),
                    parent_id: row.get(8)?,
                    created_at: millis_string(row.get(6)?),
                    updated_at: millis_string(row.get(7)?),
                })
            })
            .map_err(sql_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
    }

    fn inspect_codex(&self, id: &str) -> Result<AgentSessionSnapshot, AgentError> {
        let db_path = find_codex_state_db(&self.codex_home)?
            .ok_or_else(|| AgentError::not_found("Codex session storage was not found"))?;
        let connection = open_read_only(&db_path)?;
        let row = connection
            .query_row(
                "SELECT t.id, t.title, t.cwd, t.model, t.source, t.archived, \
                        COALESCE(t.created_at_ms, t.created_at * 1000), \
                        COALESCE(t.updated_at_ms, t.updated_at * 1000), e.parent_thread_id, t.rollout_path \
                 FROM threads t LEFT JOIN thread_spawn_edges e ON e.child_thread_id = t.id WHERE t.id = ?1",
                params![id],
                |row| {
                    let summary = AgentSessionSummary {
                        id: row.get(0)?,
                        backend: AgentSessionBackend::Codex,
                        title: non_empty(row.get(1)?),
                        cwd: non_empty(row.get(2)?),
                        model: row.get(3)?,
                        source: non_empty(row.get(4)?),
                        status: Some(
                            if row.get::<_, i64>(5)? == 0 {
                                "active"
                            } else {
                                "archived"
                            }
                            .to_owned(),
                        ),
                        parent_id: row.get(8)?,
                        created_at: millis_string(row.get(6)?),
                        updated_at: millis_string(row.get(7)?),
                    };
                    Ok((summary, PathBuf::from(row.get::<_, String>(9)?)))
                },
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| AgentError::not_found(format!("Codex thread '{id}' was not found")))?;

        let mut children_statement = connection
            .prepare(
                "SELECT t.id, t.title, t.cwd, t.model, t.source, e.status, \
                        COALESCE(t.created_at_ms, t.created_at * 1000), \
                        COALESCE(t.updated_at_ms, t.updated_at * 1000) \
                 FROM thread_spawn_edges e JOIN threads t ON t.id = e.child_thread_id \
                 WHERE e.parent_thread_id = ?1 ORDER BY t.created_at ASC",
            )
            .map_err(sql_error)?;
        let children = children_statement
            .query_map(params![id], |child| {
                Ok(AgentSessionSummary {
                    id: child.get(0)?,
                    backend: AgentSessionBackend::Codex,
                    title: non_empty(child.get(1)?),
                    cwd: non_empty(child.get(2)?),
                    model: child.get(3)?,
                    source: non_empty(child.get(4)?),
                    status: child.get(5)?,
                    parent_id: Some(id.to_owned()),
                    created_at: millis_string(child.get(6)?),
                    updated_at: millis_string(child.get(7)?),
                })
            })
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        let (turns, truncated) = parse_codex_rollout(&row.1)?;
        let child_task = match row.0.parent_id.as_deref() {
            Some(parent_id) => codex_child_task(&connection, parent_id, &turns)?,
            None => None,
        };
        Ok(AgentSessionSnapshot {
            session: row.0,
            turns,
            children,
            child_task,
            truncated,
        })
    }

    fn list_opencode(&self, scope: AgentSessionScope, limit: usize) -> Result<Vec<AgentSessionSummary>, AgentError> {
        let Some(db_path) = opencode_db_path(&self.opencode_data_dir) else {
            return Ok(Vec::new());
        };
        let connection = open_read_only(&db_path)?;
        let columns = sqlite_table_columns(&connection, "session")?;
        if !columns.contains("id") {
            return Ok(Vec::new());
        }
        let scope_clause = match (scope, columns.contains("parent_id")) {
            (AgentSessionScope::All, _) | (AgentSessionScope::Main, false) => "",
            (AgentSessionScope::Main, true) => "WHERE parent_id IS NULL",
            (AgentSessionScope::Child, true) => "WHERE parent_id IS NOT NULL",
            (AgentSessionScope::Child, false) => return Ok(Vec::new()),
        };
        let optional_column = |name: &'static str| if columns.contains(name) { name } else { "NULL" };
        let order_column = if columns.contains("time_updated") {
            "time_updated"
        } else if columns.contains("time_created") {
            "time_created"
        } else {
            "id"
        };
        let sql = format!(
            "SELECT id, {}, {}, {}, {}, {}, {}, {}, {} \
             FROM session {scope_clause} ORDER BY {order_column} DESC LIMIT ?1",
            optional_column("title"),
            optional_column("directory"),
            optional_column("model"),
            optional_column("agent"),
            optional_column("time_archived"),
            optional_column("parent_id"),
            optional_column("time_created"),
            optional_column("time_updated"),
        );
        let mut statement = connection.prepare(&sql).map_err(sql_error)?;
        let rows = statement
            .query_map(params![limit as i64], opencode_summary_from_row)
            .map_err(sql_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
    }

    fn inspect_opencode(&self, id: &str) -> Result<AgentSessionSnapshot, AgentError> {
        let db_path = opencode_db_path(&self.opencode_data_dir)
            .ok_or_else(|| AgentError::not_found("OpenCode session storage was not found"))?;
        let connection = open_read_only(&db_path)?;
        let session = connection
            .query_row(
                "SELECT id, title, directory, model, agent, time_archived, parent_id, time_created, time_updated \
                 FROM session WHERE id = ?1",
                params![id],
                opencode_summary_from_row,
            )
            .optional()
            .map_err(sql_error)?
            .ok_or_else(|| AgentError::not_found(format!("OpenCode session '{id}' was not found")))?;

        let mut children_statement = connection
            .prepare(
                "SELECT id, title, directory, model, agent, time_archived, parent_id, time_created, time_updated \
                 FROM session WHERE parent_id = ?1 ORDER BY time_created ASC",
            )
            .map_err(sql_error)?;
        let children = children_statement
            .query_map(params![id], opencode_summary_from_row)
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        let (turns, truncated) = parse_opencode_session(&connection, id)?;
        Ok(AgentSessionSnapshot {
            session,
            turns,
            children,
            child_task: None,
            truncated,
        })
    }

    fn list_pi(&self, scope: AgentSessionScope, limit: usize) -> Result<Vec<AgentSessionSummary>, AgentError> {
        if scope == AgentSessionScope::Child {
            return Ok(Vec::new());
        }
        let session_dir = self.pi_agent_dir.join("sessions");
        if !session_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for path in pi_session_files(&session_dir) {
            if let Some(summary) = parse_pi_summary(&path)? {
                sessions.push(summary);
            }
        }
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    fn inspect_pi(&self, id: &str) -> Result<AgentSessionSnapshot, AgentError> {
        let session_dir = self.pi_agent_dir.join("sessions");
        for path in pi_session_files(&session_dir) {
            let Some(summary) = parse_pi_summary(&path)? else {
                continue;
            };
            if summary.id == id {
                let (turns, truncated) = parse_pi_session(&path)?;
                return Ok(AgentSessionSnapshot {
                    session: summary,
                    turns,
                    children: Vec::new(),
                    child_task: None,
                    truncated,
                });
            }
        }
        Err(AgentError::not_found(format!("Pi session '{id}' was not found")))
    }
}

fn validate_session_id(id: &str) -> Result<(), AgentError> {
    if id.is_empty()
        || id.len() > 200
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(AgentError::bad_request("Invalid CLI session ID"));
    }
    Ok(())
}

fn find_codex_state_db(home: &Path) -> Result<Option<PathBuf>, AgentError> {
    let entries = match fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("read Codex home", error)),
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("state_") && name.ends_with(".sqlite"))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.strip_prefix("state_"))
            .and_then(|version| version.parse::<u64>().ok())
            .unwrap_or_default()
    });
    Ok(candidates.pop())
}

fn open_read_only(path: &Path) -> Result<Connection, AgentError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(sql_error)
}

fn opencode_db_path(data_dir: &Path) -> Option<PathBuf> {
    if data_dir.is_file() {
        return Some(data_dir.to_owned());
    }
    let path = data_dir.join("opencode.db");
    path.is_file().then_some(path)
}

fn sqlite_table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>, AgentError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sql_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(sql_error)?;
    Ok(columns)
}

fn opencode_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSessionSummary> {
    let raw_model = row.get::<_, Option<String>>(3)?;
    let agent = row.get::<_, Option<String>>(4)?;
    Ok(AgentSessionSummary {
        id: row.get(0)?,
        backend: AgentSessionBackend::Opencode,
        title: row.get::<_, Option<String>>(1)?.and_then(non_empty),
        cwd: row.get::<_, Option<String>>(2)?.and_then(non_empty),
        model: raw_model.and_then(|value| opencode_model_name(&value)),
        source: agent
            .map(|agent| format!("opencode/{agent}"))
            .or_else(|| Some("opencode_cli".to_owned())),
        status: Some(
            if row.get::<_, Option<i64>>(5)?.is_some() {
                "archived"
            } else {
                "active"
            }
            .to_owned(),
        ),
        parent_id: row.get(6)?,
        created_at: millis_string(row.get(7)?),
        updated_at: millis_string(row.get(8)?),
    })
}

fn opencode_model_name(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let model = value
        .get("id")
        .or_else(|| value.get("modelID"))
        .and_then(Value::as_str)?;
    let provider = value.get("providerID").and_then(Value::as_str);
    Some(provider.map_or_else(|| model.to_owned(), |provider| format!("{provider}/{model}")))
}

fn parse_codex_rollout(path: &Path) -> Result<(Vec<AgentSessionTurn>, bool), AgentError> {
    let file = File::open(path).map_err(|error| io_error("open Codex rollout", error))?;
    let mut turns = Vec::<AgentSessionTurn>::new();
    let mut tool_indexes = HashMap::<String, (usize, usize)>::new();
    let mut truncated = false;

    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| io_error("read Codex rollout", error))?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timestamp = entry.get("timestamp").and_then(Value::as_str).map(str::to_owned);
        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or_default();
        let payload = &entry["payload"];
        let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or_default();

        if entry_type == "event_msg" && payload_type == "task_started" {
            let id = payload
                .get("turn_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("turn-{}", turns.len() + 1));
            turns.push(AgentSessionTurn {
                id,
                model: None,
                started_at: payload
                    .get("started_at")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(timestamp),
                completed_at: None,
                items: Vec::new(),
            });
            continue;
        }
        if turns.is_empty() {
            turns.push(AgentSessionTurn {
                id: "turn-1".to_owned(),
                model: None,
                started_at: timestamp.clone(),
                completed_at: None,
                items: Vec::new(),
            });
        }
        let turn_index = turns.len() - 1;

        if entry_type == "turn_context" {
            turns[turn_index].model = payload.get("model").and_then(Value::as_str).map(str::to_owned);
            continue;
        }

        if entry_type == "event_msg" {
            let item = match payload_type {
                "user_message" => text_item(AgentSessionItemKind::UserMessage, &payload["message"], timestamp),
                "agent_message" => text_item(AgentSessionItemKind::AgentMessage, &payload["message"], timestamp),
                "agent_reasoning" => text_item(AgentSessionItemKind::Thinking, &payload["text"], timestamp),
                "task_complete" | "turn_aborted" => {
                    turns[turn_index].completed_at = timestamp;
                    None
                }
                _ => None,
            };
            if let Some(item) = item {
                push_item(&mut turns, turn_index, item, &mut truncated);
            }
            continue;
        }

        if entry_type != "response_item" {
            continue;
        }
        match payload_type {
            "function_call" | "custom_tool_call" => {
                let id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let (input, input_truncated) =
                    clipped_value(payload.get("arguments").or_else(|| payload.get("input")).cloned());
                let item = AgentSessionItem {
                    kind: AgentSessionItemKind::ToolCall,
                    id: id.clone(),
                    name: payload.get("name").and_then(Value::as_str).map(str::to_owned),
                    text: None,
                    input,
                    output: None,
                    status: Some("running".to_owned()),
                    timestamp,
                    truncated: input_truncated,
                };
                let item_index = turns[turn_index].items.len();
                push_item(&mut turns, turn_index, item, &mut truncated);
                if let Some(id) = id {
                    tool_indexes.insert(id, (turn_index, item_index));
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let id = payload.get("call_id").and_then(Value::as_str).unwrap_or_default();
                if let Some(&(tool_turn, item_index)) = tool_indexes.get(id) {
                    let (output, output_truncated) = clipped_value(payload.get("output").cloned());
                    if let Some(item) = turns.get_mut(tool_turn).and_then(|turn| turn.items.get_mut(item_index)) {
                        item.output = output;
                        item.status = Some("completed".to_owned());
                        item.truncated |= output_truncated;
                        truncated |= output_truncated;
                    }
                }
            }
            _ => {}
        }
    }
    Ok((turns, truncated))
}

#[derive(Debug)]
struct CodexSpawnDispatch {
    prompt: String,
    agent_type: Option<String>,
    fork_context: Option<bool>,
}

/// Build child-task metadata without assuming a thread edge identifies a
/// particular `spawn_agent` call. Codex persists parent/child thread IDs but
/// not the call ID, so the dispatch options are exposed only for one exact
/// prompt match. The child-received prompt remains useful on its own.
fn codex_child_task(
    connection: &Connection,
    parent_id: &str,
    child_turns: &[AgentSessionTurn],
) -> Result<Option<AgentSessionChildTask>, AgentError> {
    let Some(initial_item) = child_turns
        .iter()
        .flat_map(|turn| turn.items.iter())
        .find(|item| item.kind == AgentSessionItemKind::UserMessage && item.text.is_some())
    else {
        return Ok(None);
    };
    let prompt = initial_item.text.clone().unwrap_or_default();
    if initial_item.truncated {
        return Ok(Some(child_task_from_dispatches(&prompt, true, &[])));
    }
    let parent_rollout = connection
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            params![parent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?;
    let Some(parent_rollout) = parent_rollout else {
        return Ok(Some(child_task_from_dispatches(&prompt, false, &[])));
    };
    let dispatches = match parse_codex_spawn_dispatches(Path::new(&parent_rollout)) {
        Ok(dispatches) => dispatches,
        Err(_) => return Ok(Some(child_task_from_dispatches(&prompt, false, &[]))),
    };
    Ok(Some(child_task_from_dispatches(&prompt, false, &dispatches)))
}

fn child_task_from_dispatches(
    prompt: &str,
    prompt_truncated: bool,
    dispatches: &[CodexSpawnDispatch],
) -> AgentSessionChildTask {
    let mut task = AgentSessionChildTask {
        prompt: prompt.to_owned(),
        agent_type: None,
        fork_context: None,
    };
    if prompt_truncated {
        return task;
    }
    let matches = dispatches
        .iter()
        .filter(|dispatch| dispatch.prompt == prompt)
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        task.agent_type.clone_from(&matches[0].agent_type);
        task.fork_context = matches[0].fork_context;
    }
    task
}

fn parse_codex_spawn_dispatches(path: &Path) -> Result<Vec<CodexSpawnDispatch>, AgentError> {
    let file = File::open(path).map_err(|error| io_error("open Codex parent rollout", error))?;
    let mut dispatches = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| io_error("read Codex parent rollout", error))?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &entry["payload"];
        if entry.get("type").and_then(Value::as_str) != Some("response_item")
            || payload.get("type").and_then(Value::as_str) != Some("function_call")
            || payload.get("name").and_then(Value::as_str) != Some("spawn_agent")
        {
            continue;
        }
        let Some(arguments) = payload.get("arguments").and_then(Value::as_str) else {
            continue;
        };
        let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
            continue;
        };
        let Some(prompt) = arguments.get("message").and_then(Value::as_str) else {
            continue;
        };
        dispatches.push(CodexSpawnDispatch {
            prompt: prompt.to_owned(),
            agent_type: arguments.get("agent_type").and_then(Value::as_str).map(str::to_owned),
            fork_context: arguments.get("fork_context").and_then(Value::as_bool),
        });
    }
    Ok(dispatches)
}

fn parse_opencode_session(
    connection: &Connection,
    session_id: &str,
) -> Result<(Vec<AgentSessionTurn>, bool), AgentError> {
    let mut message_statement = connection
        .prepare(
            "SELECT id, time_created, time_updated, data FROM message \
             WHERE session_id = ?1 ORDER BY time_created ASC, id ASC",
        )
        .map_err(sql_error)?;
    let messages = message_statement
        .query_map(params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    drop(message_statement);

    let mut part_statement = connection
        .prepare(
            "SELECT id, time_created, data FROM part \
             WHERE session_id = ?1 AND message_id = ?2 ORDER BY time_created ASC, id ASC",
        )
        .map_err(sql_error)?;
    let mut turns = Vec::<AgentSessionTurn>::new();
    let mut truncated = false;
    for (message_id, created_at, updated_at, raw_message) in messages {
        let Ok(message) = serde_json::from_str::<Value>(&raw_message) else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
        if role == "user" || turns.is_empty() {
            turns.push(AgentSessionTurn {
                id: message_id.clone(),
                model: None,
                started_at: Some(created_at.to_string()),
                completed_at: None,
                items: Vec::new(),
            });
        }
        let turn_index = turns.len() - 1;
        let parts = part_statement
            .query_map(params![session_id, message_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        for (part_id, part_created_at, raw_part) in parts {
            let Ok(part) = serde_json::from_str::<Value>(&raw_part) else {
                continue;
            };
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
            let timestamp = part
                .pointer("/time/start")
                .and_then(Value::as_i64)
                .unwrap_or(part_created_at)
                .to_string();
            match part_type {
                "text" if role == "user" || role == "assistant" => {
                    let Some(text) = part.get("text").and_then(Value::as_str) else {
                        continue;
                    };
                    let (text, item_truncated) = truncate_text(text);
                    if text.is_empty() {
                        continue;
                    }
                    push_item(
                        &mut turns,
                        turn_index,
                        AgentSessionItem {
                            kind: if role == "user" {
                                AgentSessionItemKind::UserMessage
                            } else {
                                AgentSessionItemKind::AgentMessage
                            },
                            id: Some(part_id),
                            name: None,
                            text: Some(text),
                            input: None,
                            output: None,
                            status: None,
                            timestamp: Some(timestamp),
                            truncated: item_truncated,
                        },
                        &mut truncated,
                    );
                }
                "reasoning" => {
                    if let Some(item) = text_item(AgentSessionItemKind::Thinking, &part["text"], Some(timestamp)) {
                        push_item(&mut turns, turn_index, item, &mut truncated);
                    }
                }
                "tool" => {
                    let state = &part["state"];
                    let (input, input_truncated) = clipped_value(state.get("input").cloned());
                    let output_value = state
                        .get("output")
                        .cloned()
                        .or_else(|| state.get("error").cloned())
                        .or_else(|| state.get("metadata").cloned());
                    let (output, output_truncated) = clipped_value(output_value);
                    push_item(
                        &mut turns,
                        turn_index,
                        AgentSessionItem {
                            kind: AgentSessionItemKind::ToolCall,
                            id: part
                                .get("callID")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or(Some(part_id)),
                            name: part.get("tool").and_then(Value::as_str).map(str::to_owned),
                            text: None,
                            input,
                            output,
                            status: state.get("status").and_then(Value::as_str).map(str::to_owned),
                            timestamp: Some(timestamp),
                            truncated: input_truncated || output_truncated,
                        },
                        &mut truncated,
                    );
                }
                _ => {}
            }
        }
        if role == "assistant" {
            turns[turn_index].completed_at = message
                .pointer("/time/completed")
                .and_then(Value::as_i64)
                .map(|value| value.to_string())
                .or_else(|| Some(updated_at.to_string()));
        }
    }
    Ok((turns, truncated))
}

fn pi_session_files(session_dir: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(session_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .map(|entry| entry.into_path())
}

fn parse_pi_summary(path: &Path) -> Result<Option<AgentSessionSummary>, AgentError> {
    let file = File::open(path).map_err(|error| io_error("open Pi session", error))?;
    let mut id = None;
    let mut cwd = None;
    let mut title = None;
    let mut model = None;
    let mut created_at = None;
    let mut updated_at = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| io_error("read Pi session", error))?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timestamp = value_timestamp(&entry);
        created_at = created_at.or_else(|| timestamp.clone());
        if timestamp.is_some() {
            updated_at = timestamp;
        }
        match entry.get("type").and_then(Value::as_str) {
            Some("session") => {
                id = entry.get("id").and_then(Value::as_str).map(str::to_owned);
                cwd = entry.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            Some("model_change") => {
                model = entry.get("modelId").and_then(Value::as_str).map(str::to_owned);
            }
            Some("message") => {
                let message = &entry["message"];
                if message.get("role").and_then(Value::as_str) == Some("user") && title.is_none() {
                    title = pi_content_text(message.get("content")).map(|text| truncate_text(&text).0);
                }
                if message.get("role").and_then(Value::as_str) == Some("assistant") {
                    model = message
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or(model);
                }
            }
            _ => {}
        }
    }
    let id = id.or_else(|| pi_id_from_filename(path));
    Ok(id.map(|id| AgentSessionSummary {
        id,
        backend: AgentSessionBackend::Pi,
        title,
        cwd,
        model,
        source: Some("pi_cli".to_owned()),
        status: None,
        parent_id: None,
        created_at,
        updated_at,
    }))
}

fn parse_pi_session(path: &Path) -> Result<(Vec<AgentSessionTurn>, bool), AgentError> {
    let file = File::open(path).map_err(|error| io_error("open Pi session", error))?;
    let mut turns = Vec::<AgentSessionTurn>::new();
    let mut tool_indexes = HashMap::<String, (usize, usize)>::new();
    let mut truncated = false;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| io_error("read Pi session", error))?;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let message = &entry["message"];
        let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
        let timestamp = value_timestamp(&entry);
        if role == "user" || turns.is_empty() {
            turns.push(AgentSessionTurn {
                id: entry
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("turn-{}", turns.len() + 1)),
                model: None,
                started_at: timestamp.clone(),
                completed_at: None,
                items: Vec::new(),
            });
        }
        let turn_index = turns.len() - 1;
        if role == "user" {
            if let Some(text) = pi_content_text(message.get("content")) {
                let (text, item_truncated) = truncate_text(&text);
                push_item(
                    &mut turns,
                    turn_index,
                    AgentSessionItem {
                        kind: AgentSessionItemKind::UserMessage,
                        id: entry.get("id").and_then(Value::as_str).map(str::to_owned),
                        name: None,
                        text: Some(text),
                        input: None,
                        output: None,
                        status: None,
                        timestamp,
                        truncated: item_truncated,
                    },
                    &mut truncated,
                );
            }
            continue;
        }
        if role == "toolResult" {
            let tool_id = message.get("toolCallId").and_then(Value::as_str).unwrap_or_default();
            if let Some(&(tool_turn, item_index)) = tool_indexes.get(tool_id) {
                let (output, output_truncated) = clipped_value(message.get("content").cloned());
                if let Some(item) = turns.get_mut(tool_turn).and_then(|turn| turn.items.get_mut(item_index)) {
                    item.output = output;
                    item.status = Some(
                        if message.get("isError").and_then(Value::as_bool).unwrap_or(false) {
                            "failed"
                        } else {
                            "completed"
                        }
                        .to_owned(),
                    );
                    item.truncated |= output_truncated;
                    truncated |= output_truncated;
                }
            }
            continue;
        }
        if role != "assistant" {
            continue;
        }
        if let Some(content) = message.get("content").and_then(Value::as_array) {
            for part in content {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
                match part_type {
                    "text" | "thinking" => {
                        let (text, item_truncated) =
                            truncate_text(part.get("text").and_then(Value::as_str).unwrap_or_default());
                        if !text.is_empty() {
                            push_item(
                                &mut turns,
                                turn_index,
                                AgentSessionItem {
                                    kind: if part_type == "thinking" {
                                        AgentSessionItemKind::Thinking
                                    } else {
                                        AgentSessionItemKind::AgentMessage
                                    },
                                    id: None,
                                    name: None,
                                    text: Some(text),
                                    input: None,
                                    output: None,
                                    status: None,
                                    timestamp: timestamp.clone(),
                                    truncated: item_truncated,
                                },
                                &mut truncated,
                            );
                        }
                    }
                    "toolCall" => {
                        let id = part.get("id").and_then(Value::as_str).map(str::to_owned);
                        let (input, input_truncated) = clipped_value(part.get("arguments").cloned());
                        let item_index = turns[turn_index].items.len();
                        push_item(
                            &mut turns,
                            turn_index,
                            AgentSessionItem {
                                kind: AgentSessionItemKind::ToolCall,
                                id: id.clone(),
                                name: part.get("name").and_then(Value::as_str).map(str::to_owned),
                                text: None,
                                input,
                                output: None,
                                status: Some("running".to_owned()),
                                timestamp: timestamp.clone(),
                                truncated: input_truncated,
                            },
                            &mut truncated,
                        );
                        if let Some(id) = id {
                            tool_indexes.insert(id, (turn_index, item_index));
                        }
                    }
                    _ => {}
                }
            }
        }
        turns[turn_index].completed_at = timestamp;
    }
    Ok((turns, truncated))
}

fn push_item(turns: &mut [AgentSessionTurn], turn_index: usize, item: AgentSessionItem, truncated: &mut bool) {
    if turns.iter().map(|turn| turn.items.len()).sum::<usize>() >= MAX_ITEMS {
        *truncated = true;
        return;
    }
    *truncated |= item.truncated;
    turns[turn_index].items.push(item);
}

fn text_item(kind: AgentSessionItemKind, value: &Value, timestamp: Option<String>) -> Option<AgentSessionItem> {
    let text = value.as_str()?;
    let (text, truncated) = truncate_text(text);
    Some(AgentSessionItem {
        kind,
        id: None,
        name: None,
        text: Some(text),
        input: None,
        output: None,
        status: None,
        timestamp,
        truncated,
    })
}

fn clipped_value(value: Option<Value>) -> (Option<Value>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let value = match value {
        Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        value => value,
    };
    let serialized = value.to_string();
    if serialized.chars().count() <= MAX_VALUE_CHARS {
        return (Some(value), false);
    }
    let (preview, _) = truncate_text(&serialized);
    (Some(json!({ "preview": preview, "truncated": true })), true)
}

fn truncate_text(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_VALUE_CHARS {
        return (text.to_owned(), false);
    }
    let preview = text.chars().take(MAX_VALUE_CHARS).collect::<String>();
    (format!("{preview}\n...[truncated]"), true)
}

fn pi_content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn pi_id_from_filename(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit_once('_').map(|(_, id)| id.to_owned()))
}

fn value_timestamp(entry: &Value) -> Option<String> {
    entry
        .get("timestamp")
        .and_then(|value| match value {
            Value::String(timestamp) => Some(timestamp.clone()),
            Value::Number(timestamp) => Some(timestamp.to_string()),
            _ => None,
        })
        .or_else(|| {
            entry.pointer("/message/timestamp").and_then(|value| match value {
                Value::String(timestamp) => Some(timestamp.clone()),
                Value::Number(timestamp) => Some(timestamp.to_string()),
                _ => None,
            })
        })
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn millis_string(value: Option<i64>) -> Option<String> {
    value.map(|value| value.to_string())
}

fn sql_error(error: rusqlite::Error) -> AgentError {
    AgentError::internal(format!("Failed to read CLI session storage: {error}"))
}

fn io_error(action: &str, error: std::io::Error) -> AgentError {
    AgentError::internal(format!("Failed to {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_codex_fixture(root: &Path) -> (String, String) {
        fs::create_dir_all(root.join("sessions")).unwrap();
        let db_path = root.join("state_5.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, source TEXT NOT NULL, model_provider TEXT NOT NULL, cwd TEXT NOT NULL, title TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0, model TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER NOT NULL DEFAULT 0); \
                 CREATE TABLE thread_spawn_edges (parent_thread_id TEXT NOT NULL, child_thread_id TEXT NOT NULL PRIMARY KEY, status TEXT NOT NULL);",
            )
            .unwrap();
        let parent_id = "019f-parent".to_owned();
        let child_id = "019f-child".to_owned();
        let parent_rollout = root.join("sessions").join("parent.jsonl");
        let mut parent_file = File::create(&parent_rollout).unwrap();
        writeln!(parent_file, "{}", json!({"timestamp":"2026-08-01T00:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}})).unwrap();
        writeln!(parent_file, "{}", json!({"timestamp":"2026-08-01T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"inspect"}})).unwrap();
        writeln!(parent_file, "{}", json!({"timestamp":"2026-08-01T00:00:02Z","type":"response_item","payload":{"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"}})).unwrap();
        let spawn_args =
            json!({"message":"inspect child workspace","agent_type":"explorer","fork_context":true}).to_string();
        writeln!(parent_file, "{}", json!({"timestamp":"2026-08-01T00:00:03Z","type":"response_item","payload":{"type":"function_call","call_id":"call-spawn","name":"spawn_agent","arguments":spawn_args}})).unwrap();
        writeln!(parent_file, "{}", json!({"timestamp":"2026-08-01T00:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"ok"}})).unwrap();

        let child_rollout = root.join("sessions").join("child.jsonl");
        let mut child_file = File::create(&child_rollout).unwrap();
        writeln!(child_file, "{}", json!({"timestamp":"2026-08-01T00:00:05Z","type":"event_msg","payload":{"type":"task_started","turn_id":"child-turn-1"}})).unwrap();
        writeln!(
            child_file,
            "{}",
            json!({"timestamp":"2026-08-01T00:00:06Z","type":"turn_context","payload":{"model":"gpt-child-mini"}})
        )
        .unwrap();
        writeln!(child_file, "{}", json!({"timestamp":"2026-08-01T00:00:07Z","type":"event_msg","payload":{"type":"user_message","message":"inspect child workspace"}})).unwrap();
        writeln!(child_file, "{}", json!({"timestamp":"2026-08-01T00:00:08Z","type":"event_msg","payload":{"type":"agent_message","message":"done"}})).unwrap();

        for (id, title, model, path) in [
            (
                &parent_id,
                "Parent",
                "gpt-parent",
                parent_rollout.to_string_lossy().to_string(),
            ),
            (
                &child_id,
                "Child",
                "gpt-child",
                child_rollout.to_string_lossy().to_string(),
            ),
        ] {
            connection.execute("INSERT INTO threads (id, rollout_path, created_at, updated_at, source, model_provider, cwd, title, archived, model, created_at_ms, updated_at_ms, recency_at_ms) VALUES (?1, ?2, 1, 2, 'cli', 'openai', '/tmp', ?3, 0, ?4, 1000, 2000, 2000)", params![id, path, title, model]).unwrap();
        }
        connection
            .execute(
                "INSERT INTO thread_spawn_edges VALUES (?1, ?2, 'completed')",
                params![parent_id, child_id],
            )
            .unwrap();
        (parent_id, child_id)
    }

    fn create_opencode_fixture(root: &Path) -> (String, String) {
        let connection = Connection::open(root.join("opencode.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT NOT NULL, directory TEXT NOT NULL, model TEXT, agent TEXT, time_archived INTEGER, parent_id TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL); \
                 CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL); \
                 CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
            )
            .unwrap();
        let parent_id = "ses-parent".to_owned();
        let child_id = "ses-child".to_owned();
        for (id, title, parent) in [
            (&parent_id, "Parent", None::<&str>),
            (&child_id, "Child", Some(parent_id.as_str())),
        ] {
            connection
                .execute(
                    "INSERT INTO session VALUES (?1, ?2, '/tmp/project', '{\"id\":\"gpt-5\",\"providerID\":\"openai\"}', 'build', NULL, ?3, 1000, 2000)",
                    params![id, title, parent],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO message VALUES ('msg-user', ?1, 1100, 1100, '{\"role\":\"user\"}')",
                params![parent_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES ('msg-agent', ?1, 1200, 1500, '{\"role\":\"assistant\",\"time\":{\"completed\":1500}}')",
                params![parent_id],
            )
            .unwrap();
        for (id, message_id, created, data) in [
            ("part-user", "msg-user", 1100, json!({"type":"text","text":"inspect"})),
            (
                "part-reasoning",
                "msg-agent",
                1200,
                json!({"type":"reasoning","text":"checking"}),
            ),
            (
                "part-tool",
                "msg-agent",
                1300,
                json!({"type":"tool","tool":"bash","callID":"call-1","state":{"status":"completed","input":{"command":"pwd"},"output":"/tmp/project"}}),
            ),
            ("part-agent", "msg-agent", 1400, json!({"type":"text","text":"done"})),
        ] {
            connection
                .execute(
                    "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                    params![id, message_id, parent_id, created, data.to_string()],
                )
                .unwrap();
        }
        (parent_id, child_id)
    }

    fn create_legacy_opencode_fixture(root: &Path) {
        let connection = Connection::open(root.join("opencode.db")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT, directory TEXT); \
                 INSERT INTO session VALUES ('ses-legacy', 'Legacy', '/tmp/legacy');",
            )
            .unwrap();
    }

    #[tokio::test]
    async fn reads_codex_thread_tools_and_children() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        let (parent_id, child_id) = create_codex_fixture(codex.path());
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let main = service
            .list(AgentSessionBackend::Codex, AgentSessionScope::Main, None)
            .await
            .unwrap();
        let child = service
            .list(AgentSessionBackend::Codex, AgentSessionScope::Child, None)
            .await
            .unwrap();
        assert_eq!(main[0].id, parent_id);
        assert_eq!(child[0].parent_id.as_deref(), Some(parent_id.as_str()));

        let snapshot = service.inspect(AgentSessionBackend::Codex, parent_id).await.unwrap();
        assert_eq!(snapshot.children[0].id, child_id);
        let tool = snapshot.turns[0]
            .items
            .iter()
            .find(|item| item.kind == AgentSessionItemKind::ToolCall)
            .unwrap();
        assert_eq!(tool.name.as_deref(), Some("exec_command"));
        assert_eq!(tool.status.as_deref(), Some("completed"));
        assert_eq!(tool.output, Some(Value::String("ok".to_owned())));
    }

    #[tokio::test]
    async fn reads_codex_child_dispatch_and_turn_model() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        let (_parent_id, child_id) = create_codex_fixture(codex.path());
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let snapshot = service.inspect(AgentSessionBackend::Codex, child_id).await.unwrap();
        let task = snapshot.child_task.expect("child initial task");
        assert_eq!(task.prompt, "inspect child workspace");
        assert_eq!(task.agent_type.as_deref(), Some("explorer"));
        assert_eq!(task.fork_context, Some(true));
        assert_eq!(snapshot.session.model.as_deref(), Some("gpt-child"));
        assert_eq!(snapshot.turns[0].model.as_deref(), Some("gpt-child-mini"));
    }

    #[test]
    fn child_task_keeps_the_received_prompt_when_parent_dispatch_is_ambiguous() {
        let dispatches = vec![
            CodexSpawnDispatch {
                prompt: "same task".to_owned(),
                agent_type: Some("first".to_owned()),
                fork_context: Some(true),
            },
            CodexSpawnDispatch {
                prompt: "same task".to_owned(),
                agent_type: Some("second".to_owned()),
                fork_context: Some(false),
            },
        ];

        let task = child_task_from_dispatches("same task", false, &dispatches);
        assert_eq!(task.prompt, "same task");
        assert_eq!(task.agent_type, None);
        assert_eq!(task.fork_context, None);
    }

    #[tokio::test]
    async fn reads_opencode_main_child_and_tool_details() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        let (parent_id, child_id) = create_opencode_fixture(opencode.path());
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let children = service
            .list(AgentSessionBackend::Opencode, AgentSessionScope::Child, None)
            .await
            .unwrap();
        assert_eq!(children[0].id, child_id);
        assert_eq!(children[0].parent_id.as_deref(), Some(parent_id.as_str()));

        let snapshot = service.inspect(AgentSessionBackend::Opencode, parent_id).await.unwrap();
        assert_eq!(snapshot.children[0].id, child_id);
        let tool = snapshot.turns[0]
            .items
            .iter()
            .find(|item| item.kind == AgentSessionItemKind::ToolCall)
            .unwrap();
        assert_eq!(tool.name.as_deref(), Some("bash"));
        assert_eq!(tool.output, Some(Value::String("/tmp/project".to_owned())));
    }

    #[tokio::test]
    async fn lists_legacy_opencode_sessions_without_optional_columns() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        create_legacy_opencode_fixture(opencode.path());
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let sessions = service
            .list(AgentSessionBackend::Opencode, AgentSessionScope::All, None)
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ses-legacy");
        assert_eq!(sessions[0].model, None);
    }

    #[tokio::test]
    async fn treats_an_opencode_database_without_a_session_table_as_empty() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        Connection::open(opencode.path().join("opencode.db")).unwrap();
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let sessions = service
            .list(AgentSessionBackend::Opencode, AgentSessionScope::All, None)
            .await
            .unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn reads_pi_session_by_header_id() {
        let codex = TempDir::new().unwrap();
        let opencode = TempDir::new().unwrap();
        let pi = TempDir::new().unwrap();
        let sessions = pi.path().join("sessions").join("workspace");
        fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join("2026-08-01_session-pi.jsonl");
        let mut file = File::create(path).unwrap();
        writeln!(
            file,
            "{}",
            json!({"type":"session","id":"session-pi","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"})
        )
        .unwrap();
        writeln!(file, "{}", json!({"type":"message","id":"user-1","timestamp":"2026-08-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"run pwd"}]}})).unwrap();
        writeln!(file, "{}", json!({"type":"message","id":"assistant-1","timestamp":"2026-08-01T00:00:02Z","message":{"role":"assistant","model":"glm","content":[{"type":"toolCall","id":"tool-1","name":"bash","arguments":{"command":"pwd"}}]}})).unwrap();
        writeln!(file, "{}", json!({"type":"message","id":"result-1","timestamp":"2026-08-01T00:00:03Z","message":{"role":"toolResult","toolCallId":"tool-1","content":[{"type":"text","text":"/tmp/project"}],"isError":false}})).unwrap();
        let service = AgentSessionInspectionService::with_roots(
            codex.path().to_owned(),
            opencode.path().to_owned(),
            pi.path().to_owned(),
        );

        let snapshot = service
            .inspect(AgentSessionBackend::Pi, "session-pi".to_owned())
            .await
            .unwrap();
        assert_eq!(snapshot.session.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(snapshot.turns[0].items[1].status.as_deref(), Some("completed"));
    }
}
