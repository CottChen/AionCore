use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NativeSessionBackend {
    Codex,
    Pi,
    Opencode,
}

#[derive(Debug, Deserialize)]
pub struct NativeSessionsQuery {
    pub backend: NativeSessionBackend,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSessionItem {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NativeSessionsResponse {
    pub conversation_id: String,
    pub current_session_id: Option<String>,
    pub current_backend: Option<String>,
    pub backend: NativeSessionBackend,
    pub workspace: String,
    pub items: Vec<NativeSessionItem>,
    pub next_cursor: Option<String>,
    /// ok, missing, unsupported_schema, read_error, or partial. Never hide read failures as empty results.
    pub status: String,
}
