use std::path::PathBuf;
use std::sync::Arc;

use aionui_db::{ConversationFilters, IClientPreferenceRepository, IConversationRepository};
use aionui_file::{FileError, IUploadWorkspaceResolver};

const SAVE_UPLOAD_TO_WORKSPACE_KEY: &str = "upload.saveToWorkspace";

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
            .get_by_keys(&[SAVE_UPLOAD_TO_WORKSPACE_KEY])
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
            .get(conversation_id)
            .await
            .map_err(|error| FileError::Internal(format!("failed to resolve upload conversation: {error}")))?
            .filter(|row| row.user_id == user_id)
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

    async fn authorize_workspace(&self, user_id: &str, workspace: &std::path::Path) -> Result<(), FileError> {
        let requested =
            std::fs::canonicalize(workspace).map_err(|_| FileError::NotFound("workspace not found".to_owned()))?;
        let mut cursor = None;

        loop {
            let page = self
                .conversation_repo
                .list_paginated(
                    user_id,
                    &ConversationFilters {
                        cursor: cursor.clone(),
                        limit: 100,
                        ..ConversationFilters::default()
                    },
                )
                .await
                .map_err(|error| FileError::Internal(format!("failed to authorize workspace: {error}")))?;

            for row in &page.items {
                let Ok(extra) = serde_json::from_str::<serde_json::Value>(&row.extra) else {
                    continue;
                };
                let Some(path) = extra
                    .get("workspace")
                    .or_else(|| extra.get("workspacePath"))
                    .or_else(|| extra.get("workspace_path"))
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                if std::fs::canonicalize(path).is_ok_and(|owned| owned == requested) {
                    return Ok(());
                }
            }

            if !page.has_more {
                break;
            }
            cursor = page.items.last().map(|row| row.id.clone());
            if cursor.is_none() {
                break;
            }
        }

        Err(FileError::NotFound("workspace not found".to_owned()))
    }
}
