use std::path::PathBuf;
use std::sync::Arc;

use aionui_db::{IClientPreferenceRepository, IConversationRepository};
use aionui_file::{FileError, IUploadWorkspaceResolver};

const SAVE_UPLOAD_TO_WORKSPACE_KEY: &str = "upload.saveToWorkspace";
const SYSTEM_USER_ID: &str = "system_default_user";

pub struct AppUploadWorkspaceResolver {
    conversation_repo: Arc<dyn IConversationRepository>,
    preference_repo: Arc<dyn IClientPreferenceRepository>,
}

impl AppUploadWorkspaceResolver {
    pub fn new(
        conversation_repo: Arc<dyn IConversationRepository>,
        preference_repo: Arc<dyn IClientPreferenceRepository>,
    ) -> Self {
        Self {
            conversation_repo,
            preference_repo,
        }
    }

    async fn save_to_workspace_enabled(&self) -> Result<bool, FileError> {
        let rows = self
            .preference_repo
            .get_by_keys(SYSTEM_USER_ID, &[SAVE_UPLOAD_TO_WORKSPACE_KEY])
            .await
            .map_err(|error| FileError::Internal(format!("failed to read upload preference: {error}")))?;

        Ok(rows
            .first()
            .and_then(|row| serde_json::from_str::<bool>(&row.value).ok())
            .unwrap_or(false))
    }
}

#[async_trait::async_trait]
impl IUploadWorkspaceResolver for AppUploadWorkspaceResolver {
    async fn resolve_workspace(
        &self,
        user_id: &str,
        conversation_id: &str,
        force_workspace: bool,
    ) -> Result<Option<PathBuf>, FileError> {
        if !force_workspace && !self.save_to_workspace_enabled().await? {
            return Ok(None);
        }

        let row = self
            .conversation_repo
            .get(user_id, conversation_id)
            .await
            .map_err(|error| FileError::Internal(format!("failed to resolve upload conversation: {error}")))?
            .ok_or_else(|| FileError::NotFound("conversation not found".to_owned()))?;

        let extra: serde_json::Value = serde_json::from_str(&row.extra)
            .map_err(|error| FileError::Internal(format!("invalid conversation workspace metadata: {error}")))?;
        let workspace = extra
            .get("workspace")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| FileError::BadRequest("conversation workspace not found".to_owned()))?;

        Ok(Some(PathBuf::from(workspace)))
    }
}
