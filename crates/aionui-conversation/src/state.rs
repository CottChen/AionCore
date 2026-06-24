use std::sync::Arc;

use crate::service::ConversationService;
use aionui_ai_agent::IWorkerTaskManager;
use aionui_db::IUserRepository;

/// Shared state for conversation route handlers.
#[derive(Clone)]
pub struct ConversationRouterState {
    pub service: ConversationService,
    pub task_manager: Arc<dyn IWorkerTaskManager>,
    pub user_repo: Arc<dyn IUserRepository>,
}
