use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use tracing::{info, warn};

use crate::DbError;

const CODEX_FULL_ACCESS_MIGRATION_VERSION: i64 = 21;
const USER_SCOPE_MIGRATION_VERSION: i64 = 30;
const ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION: i64 = 900023;
const ASSISTANT_WORKSPACE_BACKUP_TABLE: &str = "_aionui_assistant_workspace_upgrade_backup";

pub(crate) async fn prepare(pool: &SqlitePool, migrator: &Migrator) -> Result<(), DbError> {
    let assistant_columns_exist = assistant_workspace_columns_exist(pool).await?;
    let overlay_table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='assistant_user_overlays'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;
    let preferences_table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='user_client_preferences'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;

    if !overlay_table_exists || !preferences_table_exists {
        return Ok(());
    }

    ensure_migrations_table(pool).await?;
    align_migration_checksum(pool, migrator, CODEX_FULL_ACCESS_MIGRATION_VERSION).await?;

    let user_scope_applied = migration_applied(pool, USER_SCOPE_MIGRATION_VERSION).await?;
    let workspace_overlay_applied = migration_applied(pool, ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION).await?;

    if !user_scope_applied {
        if assistant_columns_exist {
            backup_assistant_workspace_values(pool).await?;
        }
        if workspace_overlay_applied {
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version = ?")
                .bind(ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION)
                .execute(pool)
                .await
                .map_err(DbError::Query)?;
            info!(
                migration = ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION,
                "Deferred assistant workspace overlay migration until after user-scope rebuild"
            );
        }
    } else if workspace_overlay_applied && !assistant_columns_exist {
        ensure_assistant_workspace_overlay_schema(pool).await?;
        warn!(
            migration = ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION,
            "Repaired assistant workspace schema removed by an earlier upgrade"
        );
    }

    Ok(())
}

pub(crate) async fn finalize(pool: &SqlitePool) -> Result<(), DbError> {
    let backup_exists: bool =
        sqlx::query_scalar("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?")
            .bind(ASSISTANT_WORKSPACE_BACKUP_TABLE)
            .fetch_one(pool)
            .await
            .map_err(DbError::Query)?;
    if !backup_exists {
        return Ok(());
    }

    ensure_assistant_workspace_overlay_schema(pool).await?;
    let restored = sqlx::query(&format!(
        r#"
UPDATE assistant_definitions
SET default_workspace_mode = (
        SELECT backup.default_workspace_mode
        FROM {ASSISTANT_WORKSPACE_BACKUP_TABLE} AS backup
        WHERE backup.assistant_definition_id = assistant_definitions.id
    ),
    default_workspace_value = (
        SELECT backup.default_workspace_value
        FROM {ASSISTANT_WORKSPACE_BACKUP_TABLE} AS backup
        WHERE backup.assistant_definition_id = assistant_definitions.id
    )
WHERE id IN (SELECT assistant_definition_id FROM {ASSISTANT_WORKSPACE_BACKUP_TABLE})
        "#
    ))
    .execute(pool)
    .await
    .map_err(DbError::Query)?;
    sqlx::query(&format!("DROP TABLE {ASSISTANT_WORKSPACE_BACKUP_TABLE}"))
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    info!(
        rows = restored.rows_affected(),
        "Restored assistant workspace settings after user-scope migration"
    );
    Ok(())
}

async fn migration_applied(pool: &SqlitePool, version: i64) -> Result<bool, DbError> {
    sqlx::query_scalar("SELECT COUNT(*) > 0 FROM _sqlx_migrations WHERE version = ? AND success = 1")
        .bind(version)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)
}

async fn backup_assistant_workspace_values(pool: &SqlitePool) -> Result<(), DbError> {
    sqlx::query(&format!(
        r#"
CREATE TABLE IF NOT EXISTS {ASSISTANT_WORKSPACE_BACKUP_TABLE} (
    assistant_definition_id TEXT PRIMARY KEY NOT NULL,
    default_workspace_mode  TEXT NOT NULL,
    default_workspace_value TEXT
)
        "#
    ))
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    sqlx::query(&format!(
        r#"
INSERT OR REPLACE INTO {ASSISTANT_WORKSPACE_BACKUP_TABLE} (
    assistant_definition_id, default_workspace_mode, default_workspace_value
)
SELECT id, default_workspace_mode, default_workspace_value
FROM assistant_definitions
        "#
    ))
    .execute(pool)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

async fn assistant_workspace_columns_exist(pool: &SqlitePool) -> Result<bool, DbError> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='assistant_definitions'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;
    if !table_exists {
        return Ok(false);
    }

    let has_mode: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('assistant_definitions') WHERE name = 'default_workspace_mode'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;
    let has_value: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('assistant_definitions') WHERE name = 'default_workspace_value'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;

    Ok(has_mode && has_value)
}

async fn ensure_assistant_workspace_overlay_schema(pool: &SqlitePool) -> Result<(), DbError> {
    let has_mode: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('assistant_definitions') WHERE name = 'default_workspace_mode'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;
    if !has_mode {
        sqlx::query(
            "ALTER TABLE assistant_definitions ADD COLUMN default_workspace_mode TEXT NOT NULL DEFAULT 'auto' CHECK (default_workspace_mode IN ('auto', 'fixed'))",
        )
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    }

    let has_value: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('assistant_definitions') WHERE name = 'default_workspace_value'",
    )
    .fetch_one(pool)
    .await
    .map_err(DbError::Query)?;
    if !has_value {
        sqlx::query("ALTER TABLE assistant_definitions ADD COLUMN default_workspace_value TEXT")
            .execute(pool)
            .await
            .map_err(DbError::Query)?;
    }

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS assistant_user_overlays (
    user_id                 TEXT    NOT NULL,
    assistant_definition_id TEXT    NOT NULL,
    enabled                 INTEGER,
    sort_order              INTEGER,
    agent_id_override       TEXT,
    last_used_at            INTEGER,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    PRIMARY KEY (user_id, assistant_definition_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (assistant_definition_id) REFERENCES assistant_definitions(id) ON DELETE CASCADE
)
        "#,
    )
    .execute(pool)
    .await
    .map_err(DbError::Query)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_assistant_user_overlays_user ON assistant_user_overlays(user_id)")
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_assistant_user_overlays_sort_order ON assistant_user_overlays(user_id, sort_order)",
    )
    .execute(pool)
    .await
    .map_err(DbError::Query)?;

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS user_client_preferences (
    user_id    TEXT    NOT NULL,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
)
        "#,
    )
    .execute(pool)
    .await
    .map_err(DbError::Query)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_user_client_preferences_user ON user_client_preferences(user_id)")
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
    Ok(())
}

async fn ensure_migrations_table(pool: &SqlitePool) -> Result<(), DbError> {
    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)
        "#,
    )
    .execute(pool)
    .await
    .map_err(DbError::Query)?;
    Ok(())
}

async fn align_migration_checksum(pool: &SqlitePool, migrator: &Migrator, version: i64) -> Result<bool, DbError> {
    let Some(migration) = migrator.iter().find(|migration| migration.version == version) else {
        return Ok(false);
    };

    let updated = sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ? AND success = 1")
        .bind(&*migration.checksum)
        .bind(version)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;

    if updated.rows_affected() > 0 {
        info!(version, "Aligned checksum for reconciled assistant workspace migration");
    }
    Ok(updated.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use sqlx::Sqlite;
    use sqlx::pool::PoolOptions;

    use super::*;

    #[tokio::test]
    async fn reconciles_bad_fork_migration_21_without_pre_recording_900023() {
        let pool = PoolOptions::<Sqlite>::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"
CREATE TABLE assistant_definitions (
    id TEXT PRIMARY KEY,
    default_workspace_mode TEXT NOT NULL DEFAULT 'auto',
    default_workspace_value TEXT
);
CREATE TABLE assistant_user_overlays (
    user_id TEXT NOT NULL,
    assistant_definition_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, assistant_definition_id)
);
CREATE TABLE user_client_preferences (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key)
);
CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
VALUES (21, 'assistant workspace and user overlays', TRUE, x'626164', 0);
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        prepare(&pool, &crate::database::DB_MIGRATOR).await.unwrap();

        let migration_21 = crate::database::DB_MIGRATOR
            .iter()
            .find(|migration| migration.version == CODEX_FULL_ACCESS_MIGRATION_VERSION)
            .unwrap();
        let checksum_21: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 21")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(checksum_21, &*migration_21.checksum);

        let overlay_recorded: bool =
            sqlx::query_scalar("SELECT COUNT(*) > 0 FROM _sqlx_migrations WHERE version = 900023 AND success = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!overlay_recorded);

        let backup_created: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_aionui_assistant_workspace_upgrade_backup'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(backup_created);
    }
}
