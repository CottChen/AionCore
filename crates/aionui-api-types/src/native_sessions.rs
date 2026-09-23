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
    pub search: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSessionItem {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub updated_at: i64,
    pub created_at: Option<i64>,
    pub model: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSessionMessage {
    pub id: String,
    pub role: String,
    pub kind: String,
    pub text: String,
    pub timestamp: Option<i64>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NativeSessionCatalogResponse {
    pub backend: NativeSessionBackend,
    pub items: Vec<NativeSessionItem>,
    pub next_cursor: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NativeSessionDetailResponse {
    pub backend: NativeSessionBackend,
    pub session: NativeSessionItem,
    pub messages: Vec<NativeSessionMessage>,
    pub next_cursor: Option<String>,
    pub total_messages: Option<u64>,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct NativeSessionDetailQuery {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}
