use std::borrow::Cow;
use std::path::Path;

use aionui_db::init_database;
use sqlx::migrate::{Migration, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

const OLD_ASSISTANT_WORKSPACE_MIGRATION_VERSION: i64 = 21;
const USER_SCOPE_MIGRATION_VERSION: i64 = 30;
const ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION: i64 = 900023;

async fn full_migrator() -> Migrator {
    Migrator::new(Path::new("migrations")).await.unwrap()
}

async fn run_selected_migrations(pool: &sqlx::SqlitePool, full: &Migrator, predicate: impl Fn(&Migration) -> bool) {
    let migrations = full
        .migrations
        .iter()
        .filter(|migration| predicate(migration))
        .cloned()
        .collect::<Vec<_>>();
    Migrator {
        migrations: Cow::Owned(migrations),
        ignore_missing: true,
        locking: true,
        no_tx: false,
    }
    .run(pool)
    .await
    .unwrap();
}

async fn seed_legacy_fork_database(path: &Path, pre_record_workspace_migration: bool) {
    let options = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let full = full_migrator().await;

    run_selected_migrations(&pool, &full, |migration| migration.version <= 20).await;

    let mut old_workspace_migration = full
        .migrations
        .iter()
        .find(|migration| migration.version == ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION)
        .unwrap()
        .clone();
    old_workspace_migration.version = OLD_ASSISTANT_WORKSPACE_MIGRATION_VERSION;
    old_workspace_migration.description = Cow::Borrowed("assistant workspace and user overlays");
    Migrator {
        migrations: Cow::Owned(vec![old_workspace_migration]),
        ignore_missing: true,
        locking: true,
        no_tx: false,
    }
    .run(&pool)
    .await
    .unwrap();

    run_selected_migrations(&pool, &full, |migration| (22..=27).contains(&migration.version)).await;
    run_selected_migrations(&pool, &full, |migration| matches!(migration.version, 900021 | 900022)).await;

    sqlx::query(
        r#"
INSERT INTO assistant_definitions (
    id, assistant_id, source, owner_type, source_ref, name, name_i18n,
    description_i18n, avatar_type, agent_id, rule_resource_type,
    recommended_prompts, recommended_prompts_i18n, default_model_mode,
    default_permission_mode, default_skills_mode, default_skill_ids,
    custom_skill_names, default_disabled_builtin_skill_ids,
    default_mcps_mode, default_mcp_ids, default_workspace_mode,
    default_workspace_value, created_at, updated_at
) VALUES (
    'legacy-assistant-definition', 'legacy-assistant', 'user', 'user',
    'legacy-assistant', 'Legacy Assistant', '{}', '{}', 'none',
    'legacy-agent', 'none', '[]', '{}', 'auto', 'auto', 'auto', '[]',
    '[]', '[]', 'auto', '[]', 'fixed', '/legacy/workspace', 1, 1
)
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    if pre_record_workspace_migration {
        let migration = full
            .migrations
            .iter()
            .find(|migration| migration.version == ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION)
            .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (?, ?, TRUE, ?, 0)",
        )
        .bind(migration.version)
        .bind(&*migration.description)
        .bind(&*migration.checksum)
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::query("CREATE TABLE legacy_upgrade_probe (assistant_id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO legacy_upgrade_probe (assistant_id) VALUES (?)")
        .bind("legacy-assistant-definition")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

async fn assert_workspace_upgrade(path: &Path) {
    let db = init_database(path).await.unwrap();
    let pool = db.pool();

    let assistant_id: String = sqlx::query_scalar("SELECT assistant_id FROM legacy_upgrade_probe")
        .fetch_one(pool)
        .await
        .unwrap();
    let workspace: (String, Option<String>) = sqlx::query_as(
        "SELECT default_workspace_mode, default_workspace_value FROM assistant_definitions WHERE id = ?",
    )
    .bind(assistant_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(workspace, ("fixed".to_string(), Some("/legacy/workspace".to_string())));

    for version in [
        USER_SCOPE_MIGRATION_VERSION,
        ASSISTANT_WORKSPACE_OVERLAY_MIGRATION_VERSION,
    ] {
        let applied: bool =
            sqlx::query_scalar("SELECT COUNT(*) > 0 FROM _sqlx_migrations WHERE version = ? AND success = 1")
                .bind(version)
                .fetch_one(pool)
                .await
                .unwrap();
        assert!(applied, "migration {version} should be applied");
    }

    let backup_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_aionui_assistant_workspace_upgrade_backup'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!backup_exists, "temporary workspace backup should be removed");
}

#[tokio::test]
async fn upgrades_2_1_39_fork_database_without_losing_workspace_settings() {
    for pre_record_workspace_migration in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aionui-backend.db");
        seed_legacy_fork_database(&path, pre_record_workspace_migration).await;
        assert_workspace_upgrade(&path).await;
    }
}

#[tokio::test]
async fn repairs_workspace_columns_removed_by_previous_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aionui-backend.db");
    let db = init_database(&path).await.unwrap();
    let pool = db.pool().clone();

    sqlx::query("ALTER TABLE assistant_definitions DROP COLUMN default_workspace_value")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE assistant_definitions DROP COLUMN default_workspace_mode")
        .execute(&pool)
        .await
        .unwrap();
    db.close().await;

    let repaired = init_database(&path).await.unwrap();
    for column in ["default_workspace_mode", "default_workspace_value"] {
        let exists: bool =
            sqlx::query_scalar("SELECT COUNT(*) > 0 FROM pragma_table_info('assistant_definitions') WHERE name = ?")
                .bind(column)
                .fetch_one(repaired.pool())
                .await
                .unwrap();
        assert!(exists, "missing repaired column {column}");
    }
}
