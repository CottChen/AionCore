use serde::{Deserialize, Serialize};

/// Stable file identity used by the 2.1.53+ preview and message APIs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatFileRef {
    Project { pe_id: String, relative_path: String },
    Upload { path: String },
    Local { path: String },
}
