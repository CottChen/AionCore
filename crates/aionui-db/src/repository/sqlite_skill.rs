use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::{SkillImportRecordRow, SkillRow};
use crate::repository::skill::{CreateSkillImportRecordParams, ISkillRepository, UpsertSkillParams};

const DEFAULT_USER_ID: &str = "system_default_user";

/// SQLite-backed implementation of [`ISkillRepository`].
#[derive(Clone, Debug)]
pub struct SqliteSkillRepository {
    pool: SqlitePool,
}

impl SqliteSkillRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn uses_user_scoped_skills(&self) -> Result<bool, DbError> {
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'user_id'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count > 0)
    }

    async fn uses_user_scoped_import_records(&self) -> Result<bool, DbError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('skill_import_records') WHERE name = 'user_id'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    async fn upsert_legacy(&self, params: UpsertSkillParams<'_>) -> Result<SkillRow, DbError> {
        let now = aionui_common::now_ms();
        let existing = sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE name = ?")
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?;
        let id = existing
            .as_ref()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| aionui_common::generate_prefixed_id("skill"));
        let created_at = existing.as_ref().map(|row| row.created_at).unwrap_or(now);

        sqlx::query(
            "INSERT INTO skills \
                (id, name, description, path, source, enabled, deleted_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET \
                description = excluded.description, \
                path = excluded.path, \
                source = excluded.source, \
                enabled = excluded.enabled, \
                deleted_at = NULL, \
                updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(params.name)
        .bind(params.description)
        .bind(params.path)
        .bind(params.source)
        .bind(params.enabled)
        .bind(created_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE name = ?")
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("skill '{}' was not found after upsert", params.name)))
    }

    async fn upsert_for_default_user(&self, params: UpsertSkillParams<'_>) -> Result<SkillRow, DbError> {
        let now = aionui_common::now_ms();
        let existing = sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE user_id = ? AND name = ?")
            .bind(DEFAULT_USER_ID)
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?;
        let id = existing
            .as_ref()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| aionui_common::generate_prefixed_id("skill"));
        let created_at = existing.as_ref().map(|row| row.created_at).unwrap_or(now);

        sqlx::query(
            "INSERT INTO skills \
                (id, user_id, name, description, path, source, enabled, deleted_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?) \
             ON CONFLICT(user_id, name) WHERE user_id IS NOT NULL DO UPDATE SET \
                description = excluded.description, \
                path = excluded.path, \
                source = excluded.source, \
                enabled = excluded.enabled, \
                deleted_at = NULL, \
                updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(DEFAULT_USER_ID)
        .bind(params.name)
        .bind(params.description)
        .bind(params.path)
        .bind(params.source)
        .bind(params.enabled)
        .bind(created_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE user_id = ? AND name = ?")
            .bind(DEFAULT_USER_ID)
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("skill '{}' was not found after upsert", params.name)))
    }

    async fn upsert_scoped_global(&self, params: UpsertSkillParams<'_>) -> Result<SkillRow, DbError> {
        let now = aionui_common::now_ms();
        let existing = sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE user_id IS NULL AND name = ?")
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?;
        let id = existing
            .as_ref()
            .map(|row| row.id.clone())
            .unwrap_or_else(|| aionui_common::generate_prefixed_id("skill"));
        let created_at = existing.as_ref().map(|row| row.created_at).unwrap_or(now);

        sqlx::query(
            "INSERT INTO skills \
                (id, user_id, name, description, path, source, enabled, deleted_at, created_at, updated_at) \
             VALUES (?, NULL, ?, ?, ?, ?, ?, NULL, ?, ?) \
             ON CONFLICT(name) WHERE user_id IS NULL DO UPDATE SET \
                description = excluded.description, \
                path = excluded.path, \
                source = excluded.source, \
                enabled = excluded.enabled, \
                deleted_at = NULL, \
                updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(params.name)
        .bind(params.description)
        .bind(params.path)
        .bind(params.source)
        .bind(params.enabled)
        .bind(created_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE user_id IS NULL AND name = ?")
            .bind(params.name)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("global skill '{}' was not found after upsert", params.name)))
    }
}

#[async_trait::async_trait]
impl ISkillRepository for SqliteSkillRepository {
    async fn list(&self) -> Result<Vec<SkillRow>, DbError> {
        let rows = if self.uses_user_scoped_skills().await? {
            sqlx::query_as::<_, SkillRow>(
                "SELECT * FROM skills \
                 WHERE (user_id IS NULL OR user_id = ?) AND deleted_at IS NULL AND enabled = 1 \
                 ORDER BY updated_at DESC, name ASC",
            )
            .bind(DEFAULT_USER_ID)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SkillRow>(
                "SELECT * FROM skills WHERE deleted_at IS NULL AND enabled = 1 \
                 ORDER BY updated_at DESC, name ASC",
            )
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows)
    }

    async fn find_by_name(&self, name: &str) -> Result<Option<SkillRow>, DbError> {
        let row = if self.uses_user_scoped_skills().await? {
            sqlx::query_as::<_, SkillRow>(
                "SELECT * FROM skills \
                 WHERE (user_id IS NULL OR user_id = ?) AND name = ? \
                   AND deleted_at IS NULL AND enabled = 1 \
                 ORDER BY user_id IS NULL ASC \
                 LIMIT 1",
            )
            .bind(DEFAULT_USER_ID)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE name = ? AND deleted_at IS NULL AND enabled = 1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
        };
        Ok(row)
    }

    async fn find_by_name_any(&self, name: &str) -> Result<Option<SkillRow>, DbError> {
        let row = if self.uses_user_scoped_skills().await? {
            sqlx::query_as::<_, SkillRow>(
                "SELECT * FROM skills \
                 WHERE (user_id IS NULL OR user_id = ?) AND name = ? \
                 ORDER BY user_id IS NULL ASC \
                 LIMIT 1",
            )
            .bind(DEFAULT_USER_ID)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SkillRow>("SELECT * FROM skills WHERE name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
        };
        Ok(row)
    }

    async fn upsert(&self, params: UpsertSkillParams<'_>) -> Result<SkillRow, DbError> {
        if self.uses_user_scoped_skills().await? {
            self.upsert_for_default_user(params).await
        } else {
            self.upsert_legacy(params).await
        }
    }

    async fn upsert_global(&self, params: UpsertSkillParams<'_>) -> Result<SkillRow, DbError> {
        if self.uses_user_scoped_skills().await? {
            self.upsert_scoped_global(params).await
        } else {
            self.upsert_legacy(params).await
        }
    }

    async fn delete_by_name(&self, name: &str) -> Result<SkillRow, DbError> {
        let now = aionui_common::now_ms();
        let result = if self.uses_user_scoped_skills().await? {
            sqlx::query(
                "UPDATE skills SET enabled = 0, deleted_at = ?, updated_at = ? \
                 WHERE user_id = ? AND name = ? AND deleted_at IS NULL",
            )
            .bind(now)
            .bind(now)
            .bind(DEFAULT_USER_ID)
            .bind(name)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                "UPDATE skills SET enabled = 0, deleted_at = ?, updated_at = ? \
                 WHERE name = ? AND deleted_at IS NULL",
            )
            .bind(now)
            .bind(now)
            .bind(name)
            .execute(&self.pool)
            .await?
        };

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("skill '{name}'")));
        }

        self.find_by_name_any(name)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("skill '{name}'")))
    }

    async fn create_import_record(
        &self,
        params: CreateSkillImportRecordParams<'_>,
    ) -> Result<SkillImportRecordRow, DbError> {
        let id = aionui_common::generate_prefixed_id("skill_import");
        let now = aionui_common::now_ms();

        let query = if self.uses_user_scoped_import_records().await? {
            sqlx::query(
                "INSERT INTO skill_import_records \
                    (id, operation_id, source_label, source_path, source_name, skill_id, skill_name, \
                     status, error_code, error_path, actual_bytes, limit_bytes, line, column, created_at, user_id) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(params.operation_id)
            .bind(params.source_label)
            .bind(params.source_path)
            .bind(params.source_name)
            .bind(params.skill_id)
            .bind(params.skill_name)
            .bind(params.status)
            .bind(params.error_code)
            .bind(params.error_path)
            .bind(params.actual_bytes)
            .bind(params.limit_bytes)
            .bind(params.line)
            .bind(params.column)
            .bind(now)
            .bind(DEFAULT_USER_ID)
        } else {
            sqlx::query(
                "INSERT INTO skill_import_records \
                    (id, operation_id, source_label, source_path, source_name, skill_id, skill_name, \
                     status, error_code, error_path, actual_bytes, limit_bytes, line, column, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(params.operation_id)
            .bind(params.source_label)
            .bind(params.source_path)
            .bind(params.source_name)
            .bind(params.skill_id)
            .bind(params.skill_name)
            .bind(params.status)
            .bind(params.error_code)
            .bind(params.error_path)
            .bind(params.actual_bytes)
            .bind(params.limit_bytes)
            .bind(params.line)
            .bind(params.column)
            .bind(now)
        };
        query.execute(&self.pool).await?;

        let row = sqlx::query_as::<_, SkillImportRecordRow>("SELECT * FROM skill_import_records WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_import_records(&self, limit: i64) -> Result<Vec<SkillImportRecordRow>, DbError> {
        let rows = if self.uses_user_scoped_import_records().await? {
            sqlx::query_as::<_, SkillImportRecordRow>(
                "SELECT * FROM skill_import_records WHERE user_id = ? \
                 ORDER BY created_at DESC, id DESC LIMIT ?",
            )
            .bind(DEFAULT_USER_ID)
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SkillImportRecordRow>(
                "SELECT * FROM skill_import_records ORDER BY created_at DESC, id DESC LIMIT ?",
            )
            .bind(limit.max(0))
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup() -> (SqliteSkillRepository, crate::Database) {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteSkillRepository::new(db.pool().clone());
        (repo, db)
    }

    async fn setup_user_scoped() -> (SqliteSkillRepository, SqlitePool) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE skills (\
                id TEXT PRIMARY KEY NOT NULL, \
                user_id TEXT, \
                name TEXT NOT NULL, \
                description TEXT, \
                path TEXT NOT NULL, \
                source TEXT NOT NULL, \
                enabled INTEGER NOT NULL DEFAULT 1, \
                deleted_at INTEGER, \
                created_at INTEGER NOT NULL, \
                updated_at INTEGER NOT NULL\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE UNIQUE INDEX idx_skills_global_name ON skills(name) WHERE user_id IS NULL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE UNIQUE INDEX idx_skills_user_name ON skills(user_id, name) WHERE user_id IS NOT NULL")
            .execute(&pool)
            .await
            .unwrap();

        let repo = SqliteSkillRepository::new(pool.clone());
        (repo, pool)
    }

    async fn seed_scoped_duplicate(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO skills \
                (id, user_id, name, description, path, source, enabled, created_at, updated_at) \
             VALUES \
                ('user-cron', ?, 'cron', 'User cron', '/user/cron', 'user', 1, 1, 1), \
                ('global-cron', NULL, 'cron', 'Global cron', '/old/global/cron', 'builtin', 1, 1, 1)",
        )
        .bind(DEFAULT_USER_ID)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn global_upsert_updates_only_global_duplicate_with_partial_indexes() {
        let (repo, pool) = setup_user_scoped().await;
        seed_scoped_duplicate(&pool).await;

        let updated = repo
            .upsert_global(UpsertSkillParams {
                name: "cron",
                description: Some("Updated global cron"),
                path: "/new/global/cron",
                source: "builtin",
                enabled: true,
            })
            .await
            .unwrap();

        let rows = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT user_id, path FROM skills WHERE name = 'cron' ORDER BY user_id IS NULL",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(updated.id, "global-cron");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], (Some(DEFAULT_USER_ID.to_owned()), "/user/cron".to_owned()));
        assert_eq!(rows[1], (None, "/new/global/cron".to_owned()));
    }

    #[tokio::test]
    async fn scoped_lookup_and_upsert_prefer_default_user_duplicate() {
        let (repo, pool) = setup_user_scoped().await;
        seed_scoped_duplicate(&pool).await;

        let found = repo.find_by_name_any("cron").await.unwrap().unwrap();
        assert_eq!(found.id, "user-cron");

        let updated = repo
            .upsert(UpsertSkillParams {
                name: "cron",
                description: Some("Updated user cron"),
                path: "/new/user/cron",
                source: "user",
                enabled: true,
            })
            .await
            .unwrap();
        let global_path =
            sqlx::query_scalar::<_, String>("SELECT path FROM skills WHERE user_id IS NULL AND name = 'cron'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(updated.id, "user-cron");
        assert_eq!(updated.path, "/new/user/cron");
        assert_eq!(global_path, "/old/global/cron");
    }

    #[tokio::test]
    async fn upsert_restores_soft_deleted_skill() {
        let (repo, _db) = setup().await;

        let created = repo
            .upsert(UpsertSkillParams {
                name: "sample",
                description: Some("Old"),
                path: "/tmp/old",
                source: "user",
                enabled: true,
            })
            .await
            .unwrap();
        repo.delete_by_name("sample").await.unwrap();

        let restored = repo
            .upsert(UpsertSkillParams {
                name: "sample",
                description: Some("New"),
                path: "/tmp/new",
                source: "user",
                enabled: true,
            })
            .await
            .unwrap();

        assert_eq!(restored.id, created.id);
        assert_eq!(restored.description.as_deref(), Some("New"));
        assert_eq!(restored.path, "/tmp/new");
        assert_eq!(restored.deleted_at, None);
        assert!(repo.find_by_name("sample").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn list_filters_soft_deleted_skills() {
        let (repo, _db) = setup().await;

        repo.upsert(UpsertSkillParams {
            name: "active",
            description: None,
            path: "/tmp/active",
            source: "user",
            enabled: true,
        })
        .await
        .unwrap();
        repo.upsert(UpsertSkillParams {
            name: "deleted",
            description: None,
            path: "/tmp/deleted",
            source: "user",
            enabled: true,
        })
        .await
        .unwrap();
        repo.delete_by_name("deleted").await.unwrap();

        let names: Vec<_> = repo.list().await.unwrap().into_iter().map(|row| row.name).collect();
        assert_eq!(names, vec!["active"]);
        assert!(repo.find_by_name_any("deleted").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn import_records_keep_structured_error_details() {
        let (repo, _db) = setup().await;

        let row = repo
            .create_import_record(CreateSkillImportRecordParams {
                operation_id: "import_1",
                source_label: "parent-pack",
                source_path: Some("/tmp/parent-pack"),
                source_name: "beta-skill",
                skill_id: None,
                skill_name: None,
                status: "failed",
                error_code: Some("SKILL_IMPORT_FILE_TOO_LARGE"),
                error_path: Some("assets/movie.mp4"),
                actual_bytes: Some(73_400_320),
                limit_bytes: Some(10_485_760),
                line: None,
                column: None,
            })
            .await
            .unwrap();

        assert_eq!(row.operation_id, "import_1");
        assert_eq!(row.error_path.as_deref(), Some("assets/movie.mp4"));
        assert_eq!(row.actual_bytes, Some(73_400_320));
        assert_eq!(row.limit_bytes, Some(10_485_760));
        let records = repo.list_import_records(10).await.unwrap();
        assert_eq!(records.len(), 1);
    }
}
