//! HTTP router assembly for the application.

<<<<<<< HEAD
=======
mod antigravity_hook;
mod clipboard_writer;
mod fs_monitor;
>>>>>>> a621ed88 (feat(fs): add copy-absolute-path endpoint that writes the clipboard server-side (#803))
mod health;
mod routes;
mod runtime_team_tools;
mod state;
mod team_conversation_adapters;
mod trace;
mod upload_workspace_resolver;

pub use routes::{
    RouterRuntime, create_router, create_router_with_all_state, create_router_with_runtime, create_router_with_states,
};
pub use state::{
    ChannelOrchestratorComponents, ModuleStates, RouterBuildError, build_assistant_state, build_conversation_state,
    build_extension_states, build_module_states, build_ws_state,
};
