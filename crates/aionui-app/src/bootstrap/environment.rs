//! Bootstrap layers shared by non-MCP subcommands.

use std::time::Instant;

use tracing::info;

use aionui_app::AppConfig;
use aionui_db::Database;

use crate::cli::Cli;

use super::builtin_skills::materialize_builtin_skills;
use super::tracing_init::{LogGuards, init_tracing};
use super::work_dir::resolve_work_dir;
use super::{BootstrapError, BootstrapErrorCode};

const UPLOAD_MAX_SIZE_ENV: &str = "AIONUI_UPLOAD_MAX_SIZE_MB";
const MIB: usize = 1024 * 1024;

/// Resolved environment needed by all non-MCP subcommands.
pub struct ServerEnvironment {
    /// Must be held alive for the process lifetime to flush log buffers.
    pub _log_guard: LogGuards,
    pub config: AppConfig,
}

/// Layer 1: Logging + config resolution.
///
/// Cheap, synchronous, no IO beyond creating the log directory.
/// All subcommands that need logging and config should call this first.
pub fn init_environment(cli: &Cli, merged_path: &str) -> Result<ServerEnvironment, BootstrapError> {
    let log_dir = cli.log_dir.clone().unwrap_or_else(|| cli.data_dir.join("logs"));
    let log_guard = init_tracing(&log_dir, cli.log_level.as_deref())?;

    info!(
        path_segments = merged_path.split(if cfg!(windows) { ';' } else { ':' }).count(),
        path_len = merged_path.len(),
        "startup: PATH ready"
    );

    let work_dir = resolve_work_dir(cli.work_dir.clone(), &cli.data_dir);
    let upload_max_size_bytes = resolve_upload_max_size_bytes(cli.upload_max_size_mb)?;

    // SAFETY: called before any service initialization; no concurrent reads.
    unsafe {
        std::env::set_var("AIONUI_WORK_DIR", &work_dir);
    }

    let config = AppConfig {
        host: cli.host.clone(),
        port: cli.port,
        data_dir: cli.data_dir.clone(),
        work_dir,
        app_version: cli.app_version.clone(),
        local: cli.local,
        dump_prompts: cli.dump_prompts,
        recover_corrupted_database: cli.recover_corrupted_database,
        upload_max_size_bytes,
    };
    info!(
        "Running in {} mode — authentication is {}",
        if config.local { "local" } else { "remote" },
        if config.local { "disabled" } else { "enabled" }
    );
    info!(
        upload_max_size_bytes = config.upload_max_size_bytes,
        "File upload limit configured"
    );

    Ok(ServerEnvironment {
        _log_guard: log_guard,
        config,
    })
}

fn resolve_upload_max_size_bytes(cli_value_mb: Option<usize>) -> Result<usize, BootstrapError> {
    let env_value = std::env::var(UPLOAD_MAX_SIZE_ENV).ok();
    resolve_upload_max_size_bytes_from(cli_value_mb, env_value.as_deref())
}

fn resolve_upload_max_size_bytes_from(
    cli_value_mb: Option<usize>,
    env_value_mb: Option<&str>,
) -> Result<usize, BootstrapError> {
    if let Some(value_mb) = cli_value_mb {
        return upload_max_size_bytes(value_mb, "command line");
    }

    match env_value_mb {
        Some(raw) if !raw.trim().is_empty() => {
            let value_mb = raw
                .trim()
                .parse::<usize>()
                .map_err(|_| upload_max_size_error(raw, "environment"))?;
            upload_max_size_bytes(value_mb, "environment")
        }
        _ => Ok(aionui_common::constants::DEFAULT_UPLOAD_MAX_SIZE),
    }
}

fn upload_max_size_bytes(value_mb: usize, source: &'static str) -> Result<usize, BootstrapError> {
    value_mb
        .checked_mul(MIB)
        .filter(|value| *value > 0)
        .ok_or_else(|| upload_max_size_error(&value_mb.to_string(), source))
}

fn upload_max_size_error(value: &str, source: &'static str) -> BootstrapError {
    BootstrapError::new(
        BootstrapErrorCode::ConfigInvalid,
        "config.upload_max_size_mb",
        "upload max size must be a positive integer number of MiB",
    )
    .with_field("source", source)
    .with_field("value", value.trim())
}

/// Layer 2: Materialize builtin skills + initialize the database.
///
/// Requires only `data_dir`. Subcommands that need persistent state
/// (database, skill files) should call this after `init_environment`.
pub async fn init_data_layer(config: &AppConfig) -> Result<Database, BootstrapError> {
    let boot = Instant::now();

    materialize_builtin_skills(&config.data_dir).await.map_err(|e| {
        BootstrapError::new(
            BootstrapErrorCode::DataInitFailed,
            "data.builtin_skills",
            "failed to initialize application data",
        )
        .with_source(e)
        .with_field("dataDir", config.data_dir.display().to_string())
    })?;
    info!(
        elapsed_ms = boot.elapsed().as_millis(),
        "startup: builtin skills materialized"
    );

    let db_path = config.database_path();
    aionui_db::maybe_copy_legacy_database(&db_path).map_err(|e| {
        BootstrapError::new(
            BootstrapErrorCode::DataInitFailed,
            "data.legacy_db",
            "failed to initialize application data",
        )
        .with_source(e)
        .with_field("databasePath", db_path.display().to_string())
    })?;
    info!("Initializing database at {}", db_path.display());
    let database = aionui_db::init_database_staged_with_options(
        &db_path,
        aionui_db::DatabaseInitOptions {
            recover_corrupted_database: config.recover_corrupted_database,
        },
    )
    .await
    .map_err(|e| {
        let stage = e.stage();
        BootstrapError::new(
            BootstrapErrorCode::DataInitFailed,
            stage,
            "failed to initialize application data",
        )
        .with_source(e.into_source())
        .with_field("databasePath", db_path.display().to_string())
    })?;
    info!(elapsed_ms = boot.elapsed().as_millis(), "startup: database initialized");

    Ok(database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_stage_comes_from_db_boundary_error() {
        let err = aionui_db::DatabaseInitError::new(
            "database.migration",
            aionui_db::DbError::Migration(sqlx::migrate::MigrateError::VersionMismatch(42)),
        );

        assert_eq!(err.stage(), "database.migration");
    }

    #[test]
    fn database_schema_repair_stage_comes_from_db_boundary_error() {
        let err = aionui_db::DatabaseInitError::new(
            "database.schema_repair",
            aionui_db::DbError::Init("repair failed".into()),
        );

        assert_eq!(err.stage(), "database.schema_repair");
    }

    #[test]
    fn database_recoverable_corruption_stage_comes_from_db_boundary_error() {
        let err = aionui_db::DatabaseInitError::new(
            "database.recoverable_corruption",
            aionui_db::DbError::Migration(sqlx::migrate::MigrateError::ExecuteMigration(
                sqlx::Error::Protocol("database disk image is malformed".into()),
                13,
            )),
        );

        assert_eq!(err.stage(), "database.recoverable_corruption");
    }

    #[test]
    fn upload_max_size_converts_mib_to_bytes() {
        assert_eq!(upload_max_size_bytes(256, "test").unwrap(), 256 * 1024 * 1024);
    }

    #[test]
    fn upload_max_size_rejects_zero() {
        let error = upload_max_size_bytes(0, "test").unwrap_err();
        assert_eq!(error.code(), BootstrapErrorCode::ConfigInvalid);
        assert_eq!(error.stage(), "config.upload_max_size_mb");
    }

    #[test]
    fn upload_max_size_rejects_overflow() {
        let error = upload_max_size_bytes(usize::MAX, "test").unwrap_err();
        assert_eq!(error.code(), BootstrapErrorCode::ConfigInvalid);
    }

    #[test]
    fn upload_max_size_uses_cli_before_environment() {
        assert_eq!(
            resolve_upload_max_size_bytes_from(Some(64), Some("invalid")).unwrap(),
            64 * MIB
        );
    }

    #[test]
    fn upload_max_size_uses_environment_when_cli_is_absent() {
        assert_eq!(
            resolve_upload_max_size_bytes_from(None, Some("128")).unwrap(),
            128 * MIB
        );
    }

    #[test]
    fn upload_max_size_uses_default_when_not_configured() {
        assert_eq!(
            resolve_upload_max_size_bytes_from(None, None).unwrap(),
            aionui_common::constants::DEFAULT_UPLOAD_MAX_SIZE
        );
    }

    #[test]
    fn upload_max_size_rejects_invalid_environment_value() {
        let error = resolve_upload_max_size_bytes_from(None, Some("large")).unwrap_err();
        assert_eq!(error.code(), BootstrapErrorCode::ConfigInvalid);
        assert_eq!(error.stage(), "config.upload_max_size_mb");
    }
}
