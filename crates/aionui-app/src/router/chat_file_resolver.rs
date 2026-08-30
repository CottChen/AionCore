use std::path::{Component, Path, PathBuf};

use aionui_api_types::ChatFileRef;
use aionui_file::{ChatFileOperation, FileError, IChatFileResolver, ResolvedChatFile};
use sqlx::{Row, SqlitePool};
use url::Url;

pub struct AppChatFileResolver {
    pool: SqlitePool,
}

impl AppChatFileResolver {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    fn missing() -> FileError {
        FileError::NotFound("file reference could not be resolved".to_owned())
    }

    fn resolve_local(path: &str) -> Result<ResolvedChatFile, FileError> {
        let canonical = std::fs::canonicalize(path).map_err(|_| Self::missing())?;
        if !canonical.is_file() {
            return Err(Self::missing());
        }
        let root = canonical.parent().ok_or_else(Self::missing)?.to_path_buf();
        Ok(ResolvedChatFile { path: canonical, root })
    }

    async fn resolve_upload(&self, user_id: &str, is_admin: bool, path: &str) -> Result<ResolvedChatFile, FileError> {
        let candidate = std::fs::canonicalize(path).map_err(|_| Self::missing())?;
        if !candidate.is_file() {
            return Err(Self::missing());
        }

        let managed_temp = std::env::temp_dir().join("aionui");
        if is_admin && let Some(root) = canonical_root_containing(&managed_temp, &candidate) {
            return Ok(ResolvedChatFile { path: candidate, root });
        }

        let rows = sqlx::query("SELECT id, extra FROM conversations WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "failed to resolve user upload workspace");
                Self::missing()
            })?;
        for row in rows {
            let conversation_id = row.try_get::<String, _>("id").unwrap_or_default();
            if !conversation_id.is_empty()
                && let Some(root) = canonical_root_containing(&managed_temp.join(&conversation_id), &candidate)
            {
                return Ok(ResolvedChatFile { path: candidate, root });
            }
            let Ok(extra) = row.try_get::<String, _>("extra") else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&extra) else {
                continue;
            };
            let workspace = value
                .get("workspace")
                .or_else(|| value.get("workspacePath"))
                .or_else(|| value.get("workspace_path"))
                .and_then(serde_json::Value::as_str);
            if let Some(root) = workspace.and_then(|root| canonical_root_containing(Path::new(root), &candidate)) {
                return Ok(ResolvedChatFile { path: candidate, root });
            }
        }

        Err(Self::missing())
    }

    async fn resolve_project(
        &self,
        user_id: &str,
        pe_id: &str,
        relative_path: &str,
    ) -> Result<ResolvedChatFile, FileError> {
        let relative = Path::new(relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(Self::missing());
        }

        let row = sqlx::query(
            "SELECT f.resource_uri \
             FROM project_explorer pe \
             JOIN projects p ON p.project_id = pe.project_id \
             JOIN folders f ON f.folder_id = pe.folder_id \
             WHERE pe.pe_id = ? AND pe.owner_user_id = ? AND p.user_id = ?",
        )
        .bind(pe_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::debug!(error = %error, "project file schema is unavailable or incompatible");
            Self::missing()
        })?
        .ok_or_else(Self::missing)?;

        let resource_uri: String = row.try_get("resource_uri").map_err(|_| Self::missing())?;
        let root = Url::parse(&resource_uri)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .and_then(|path| std::fs::canonicalize(path).ok())
            .ok_or_else(Self::missing)?;
        let candidate = std::fs::canonicalize(root.join(relative)).map_err(|_| Self::missing())?;
        if !candidate.starts_with(&root) || !candidate.is_file() {
            return Err(Self::missing());
        }
        Ok(ResolvedChatFile { path: candidate, root })
    }
}

fn canonical_root_containing(root: &Path, candidate: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    candidate.starts_with(&root).then_some(root)
}

#[async_trait::async_trait]
impl IChatFileResolver for AppChatFileResolver {
    async fn resolve(
        &self,
        user_id: &str,
        is_admin: bool,
        file: &ChatFileRef,
        _operation: ChatFileOperation,
    ) -> Result<ResolvedChatFile, FileError> {
        match file {
            ChatFileRef::Local { path } if is_admin => Self::resolve_local(path),
            ChatFileRef::Local { path } => self.resolve_upload(user_id, false, path).await,
            ChatFileRef::Upload { path } => self.resolve_upload(user_id, is_admin, path).await,
            ChatFileRef::Project { pe_id, relative_path } => self.resolve_project(user_id, pe_id, relative_path).await,
        }
    }
}
