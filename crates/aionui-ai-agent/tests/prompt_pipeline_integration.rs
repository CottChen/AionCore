//! Integration tests for the ACP prompt pipeline.
//!
//! Unlike acp_agent_integration.rs, these tests do not exercise
//! AcpAgentManager or the JSON-RPC protocol. They construct a
//! PromptPipeline with the built-in hooks and invoke
//! pre_send against a real PromptCtx, asserting the observable
//! prompt transformation.

use std::collections::HashMap;
use std::sync::Arc;

use aionui_ai_agent::capability::prompt_pipeline::{PromptCtx, PromptPipeline};
use aionui_ai_agent::factory::acp_assembler::{AcpSessionParams, WorkspaceInfo, assemble_acp_params};
use aionui_ai_agent::manager::acp::{AcpSession, ContextRebuildPreludeHook, SessionNewPreludeHook};
use aionui_ai_agent::registry::AgentRegistry;
use aionui_ai_agent::shared_kernel::ModelId;
use aionui_ai_agent::{AcpBuildExtra, AcpSkillManager, AgentRuntime};
use aionui_db::{SqliteAgentMetadataRepository, init_database_memory};

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// Thin wrapper used by the prelude tests (no rebuilt-context transcript).
async fn fixture_params(
    backend: &str,
    preset_context: Option<&str>,
    is_custom_workspace: bool,
) -> Arc<AcpSessionParams> {
    fixture_params_with(backend, preset_context, is_custom_workspace, None).await
}

async fn fixture_params_with(
    backend: &str,
    preset_context: Option<&str>,
    is_custom_workspace: bool,
    context_rebuild: Option<&str>,
) -> Arc<AcpSessionParams> {
    let db = init_database_memory().await.unwrap();
    let repo = Arc::new(SqliteAgentMetadataRepository::new(db.pool().clone()));
    let registry = AgentRegistry::new(repo);
    registry.hydrate().await.unwrap();

    let metadata = registry
        .find_builtin_by_backend(backend)
        .await
        .expect("seeded backend row must exist");

    let config = AcpBuildExtra {
        agent_id: None,
        backend: Some(backend.to_owned()),
        cli_path: None,
        agent_name: None,
        custom_agent_id: None,
        preset_context: preset_context.map(str::to_owned),
        skills: vec![],
        preset_assistant_id: None,
        session_mode: None,
        current_model_id: None,
        thought_level: None,
        cron_job_id: None,
        team_mcp_stdio_config: None,
        mcp_server_ids: None,
        session_mcp_servers: vec![],
        user_id: None,
        context_rebuild: context_rebuild.map(|text| aionui_api_types::PendingContextRebuild {
            text: text.to_owned(),
            turns: 1,
            model_id: None,
            created_at: 0,
        }),
    };

    Arc::new(
        assemble_acp_params(
            "conv-pp-test".into(),
            WorkspaceInfo {
                path: "/tmp".into(),
                is_custom: is_custom_workspace,
            },
            metadata,
            aionui_common::CommandSpec {
                command: "/usr/bin/true".into(),
                args: vec![],
                env: vec![],
                cwd: None,
            },
            config,
            Vec::new(),
            None,
            std::env::temp_dir(),
            false,
        )
        .await,
    )
}

fn fixture_skill_manager() -> Arc<AcpSkillManager> {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = Arc::new(aionui_extension::resolve_skill_paths(tmp.path(), tmp.path()));
    // tmp dir needs to live until the test finishes.
    // mem::forget is acceptable in test code — we just don't need the Drop cleanup.
    std::mem::forget(tmp);
    AcpSkillManager::new(paths)
}

fn fixture_runtime() -> AgentRuntime {
    AgentRuntime::new("conv-pp-test", "/tmp", 64)
}

fn make_pipeline() -> PromptPipeline {
    PromptPipeline::new(vec![Arc::new(SessionNewPreludeHook)])
}

/// Mirrors the production hook order registered by `AcpAgentManager::new`.
fn make_rebuild_pipeline() -> PromptPipeline {
    PromptPipeline::new(vec![
        Arc::new(ContextRebuildPreludeHook),
        Arc::new(SessionNewPreludeHook),
    ])
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// First prompt after session/new: prelude block injected, flag consumed.
#[tokio::test(flavor = "current_thread")]
async fn brand_new_first_prompt_injects_preset_context() {
    let params = fixture_params("claude", Some("Rule A"), true).await;
    let skill_manager = fixture_skill_manager();
    let runtime = fixture_runtime();
    let mut session = AcpSession::new(None, None, HashMap::new());

    // Simulate: open_session_new just succeeded.
    session.mark_pending_session_new_prelude();

    let pipeline = make_pipeline();

    let mut ctx = PromptCtx {
        session: &mut session,
        params: &params,
        skill_manager: &skill_manager,
        runtime: &runtime,
    };

    let out = pipeline.pre_send(&mut ctx, "hello".into()).await;
    assert!(out.contains("[Assistant Rules]"), "prelude block missing: {out}");
    assert!(out.contains("Rule A"), "preset_context missing: {out}");
    assert!(out.ends_with("hello"), "user content should be at the end: {out}");

    // Flag must have been consumed.
    assert!(
        !session.take_pending_session_new_prelude(),
        "pending_session_new_prelude must be false after pre_send consumed it"
    );
}

/// Second prompt: no prelude, no reminder — pure passthrough.
#[tokio::test(flavor = "current_thread")]
async fn second_prompt_is_passthrough() {
    let params = fixture_params("claude", Some("Rule A"), true).await;
    let skill_manager = fixture_skill_manager();
    let runtime = fixture_runtime();
    let mut session = AcpSession::new(None, None, HashMap::new());
    session.mark_pending_session_new_prelude();

    let pipeline = make_pipeline();

    // First prompt consumes the flag.
    {
        let mut ctx = PromptCtx {
            session: &mut session,
            params: &params,
            skill_manager: &skill_manager,
            runtime: &runtime,
        };
        let _ = pipeline.pre_send(&mut ctx, "first".into()).await;
    }

    // Second prompt: flag already consumed.
    let mut ctx = PromptCtx {
        session: &mut session,
        params: &params,
        skill_manager: &skill_manager,
        runtime: &runtime,
    };
    let out = pipeline.pre_send(&mut ctx, "second".into()).await;
    assert_eq!(out, "second", "no prelude / no reminder expected on second turn");
}

/// Resume path: no mark_pending_session_new_prelude — prompt must be unchanged.
#[tokio::test(flavor = "current_thread")]
async fn resume_path_does_not_inject() {
    let params = fixture_params("claude", Some("Rule A"), true).await;
    let skill_manager = fixture_skill_manager();
    let runtime = fixture_runtime();

    // Resume: session opened by open_session_resume which does NOT call
    // mark_pending_session_new_prelude. The flag stays false.
    let mut session = AcpSession::new(None, None, HashMap::new());

    let pipeline = make_pipeline();

    let mut ctx = PromptCtx {
        session: &mut session,
        params: &params,
        skill_manager: &skill_manager,
        runtime: &runtime,
    };

    let out = pipeline.pre_send(&mut ctx, "continue the story".into()).await;
    assert_eq!(out, "continue the story");
}

#[tokio::test(flavor = "current_thread")]
async fn observed_model_change_does_not_inject_model_identity_reminder() {
    let params = fixture_params("claude", None, true).await;
    let skill_manager = fixture_skill_manager();
    let runtime = fixture_runtime();
    let mut session = AcpSession::new(None, None, HashMap::new());
    session.apply_observed_model(ModelId::new("claude-opus-4"));

    let pipeline = make_pipeline();

    let mut ctx = PromptCtx {
        session: &mut session,
        params: &params,
        skill_manager: &skill_manager,
        runtime: &runtime,
    };

    let out = pipeline.pre_send(&mut ctx, "who are you?".to_owned()).await;

    assert_eq!(out, "who are you?");
    assert!(!out.contains("<system-reminder>"));
    assert!(!out.contains("claude-opus-4"));
}

/// Skeleton: unlock once inject_first_message_prefix surfaces errors.
#[tokio::test(flavor = "current_thread")]
#[ignore = "SessionNewPreludeHook relies on inject_first_message_prefix which currently swallows I/O errors internally; unlocking this test requires surfacing a fallible boundary"]
async fn prelude_io_failure_emits_prompt_hook_warning() {
    // When inject_first_message_prefix exposes an error path, the hook
    // should call emit_hook_warning("session_new_prelude", ...) and
    // return the user content unchanged. Subscribers on runtime.subscribe()
    // must then receive an AgentStreamEvent::AcpPromptHookWarning whose
    // payload deserializes to AcpPromptHookWarningPayload with
    // hook == "session_new_prelude".
    let _ = fixture_params("claude", Some("ctx"), true).await;
}

/// In-place context rebuild: the captured transcript is replayed once, between
/// the preset rules and the user's message.
#[tokio::test(flavor = "current_thread")]
async fn rebuilt_session_replays_transcript_between_rules_and_user_message() {
    let transcript = "【第 1 轮】\n用户：之前的问题\n助手：之前的回答";
    let params = fixture_params_with("claude", Some("Rule A"), true, Some(transcript)).await;
    let skill_manager = fixture_skill_manager();
    let runtime = fixture_runtime();
    let mut session = AcpSession::new(None, None, HashMap::new());

    // Simulate `open_session_new` arming both one-shot blocks.
    session.mark_pending_session_new_prelude();
    session.mark_pending_context_rebuild(transcript.to_owned());

    let pipeline = make_rebuild_pipeline();
    let mut ctx = PromptCtx {
        session: &mut session,
        params: &params,
        skill_manager: &skill_manager,
        runtime: &runtime,
    };

    let out = pipeline.pre_send(&mut ctx, "新的问题".into()).await;

    let rules_at = out.find("Rule A").expect("preset rules missing");
    let transcript_at = out.find("之前的问题").expect("transcript missing");
    let user_at = out.find("新的问题").expect("user message missing");
    assert!(rules_at < transcript_at, "rules must precede the transcript: {out}");
    assert!(
        transcript_at < user_at,
        "transcript must precede the user message: {out}"
    );
    assert!(out.ends_with("新的问题"), "user content should stay last: {out}");

    assert_eq!(
        session.take_pending_context_rebuild(),
        None,
        "transcript must be one-shot"
    );
}
