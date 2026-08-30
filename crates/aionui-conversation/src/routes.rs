#![allow(clippy::disallowed_types)]

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Json, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};

use aionui_api_types::{
    ActiveCountResponse, ApiResponse, ApprovalCheckQuery, ApprovalCheckResponse, CancelConversationRequest,
    CancelConversationResponse, CloneConversationRequest, ConfirmRequest, ConfirmationListResponse,
    ConversationArtifactListResponse, ConversationArtifactResponse, ConversationListResponse,
    ConversationRatingResponse, ConversationRatingVote, ConversationResponse, CreateConversationRequest,
    EnsureConversationRuntimeResponse, ListConversationsQuery, ListMessagesQuery, MessageListResponse, MessageResponse,
    MessageSearchResponse, SearchMessagesQuery, SendMessageRequest, SendMessageResponse,
    SubmitConversationRatingRequest, UpdateConversationArtifactRequest, UpdateConversationRequest,
};
use aionui_auth::CurrentUser;
use aionui_common::{ApiError, MessagePosition, MessageType, generate_short_id, now_ms};
use sqlx::Row;

use crate::ConversationError;
use crate::state::ConversationRouterState;

impl From<ConversationError> for ApiError {
    fn from(error: ConversationError) -> Self {
        match error {
            ConversationError::NotFound { id } => ApiError::NotFound(format!("Conversation {id} not found")),
            ConversationError::MessageNotFound { id } => ApiError::NotFound(format!("Message {id} not found")),
            ConversationError::ArtifactNotFound { id } => ApiError::NotFound(format!("Artifact {id} not found")),
            ConversationError::ActiveAgentNotFound { .. } => {
                ApiError::NotFound("No active agent for this conversation".into())
            }
            ConversationError::Archived { reason, .. } => ApiError::ConversationArchived(reason),
            ConversationError::BadRequest { reason } => ApiError::BadRequest(reason),
            ConversationError::Busy { reason } => ApiError::Conflict(reason),
            ConversationError::Forbidden { reason } => ApiError::Forbidden(reason),
            ConversationError::NotFoundReason { reason } => ApiError::NotFound(reason),
            ConversationError::Unauthorized { reason } => ApiError::Unauthorized(reason),
            ConversationError::RateLimited => ApiError::RateLimited,
            ConversationError::BadGateway { reason } => ApiError::BadGateway(reason),
            ConversationError::Timeout { reason } => ApiError::Timeout(reason),
            ConversationError::ConfigConfirmationTimeout {
                conversation_id,
                option_id,
                requested,
                last_observed,
            } => ApiError::coded(
                StatusCode::GATEWAY_TIMEOUT,
                "confirmation_timeout",
                "ACP runtime did not confirm the requested config option before timeout",
                Some(serde_json::json!({
                    "conversation_id": conversation_id,
                    "option_id": option_id,
                    "requested": requested,
                    "last_observed": last_observed,
                })),
            ),
            ConversationError::ConfigUpdateInProgress {
                conversation_id,
                option_id,
                requested,
            } => ApiError::coded(
                StatusCode::CONFLICT,
                "config_update_in_progress",
                "ACP config update is already in progress",
                Some(serde_json::json!({
                    "conversation_id": conversation_id,
                    "option_id": option_id,
                    "requested": requested,
                })),
            ),
            ConversationError::TeamRuntimeRequired {
                conversation_id,
                team_id,
            } => ApiError::coded(
                StatusCode::CONFLICT,
                "TEAM_RUNTIME_REQUIRED",
                "This conversation belongs to a team; use the team runtime session",
                Some(serde_json::json!({
                    "conversation_id": conversation_id,
                    "team_id": team_id,
                })),
            ),
            ConversationError::Unprocessable { reason } => ApiError::UnprocessableEntity(reason),
            ConversationError::Internal { reason } => ApiError::Internal(reason),
            ConversationError::WorkspacePathUnavailable { path } => ApiError::WorkspacePathUnavailable(path),
            ConversationError::WorkspacePathRuntimeUnavailable { path } => {
                ApiError::WorkspacePathRuntimeUnavailable(path)
            }
            ConversationError::OpenClawGatewayUnreachable { detail } => ApiError::coded(
                StatusCode::BAD_GATEWAY,
                "USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE",
                "OpenClaw Gateway is not reachable",
                Some(serde_json::json!({
                    "detail": detail,
                    "error_kind": "openclaw_gateway_unreachable",
                    "backend": "openclaw",
                    "port": 18789
                })),
            ),
            ConversationError::Acp(_) => ApiError::BadGateway("Agent protocol error".into()),
        }
    }
}

/// Build the conversation router (CRUD + message flow + confirmation + extended operations).
///
/// All routes require authentication (applied by the caller).
pub fn conversation_routes(state: ConversationRouterState) -> Router {
    Router::new()
        .route("/api/conversations", post(create).get(list))
        .route("/api/conversations/{id}", get(get_one).patch(update).delete(delete_one))
        .route("/api/conversations/{id}/reset", post(reset))
        .route("/api/conversations/{id}/associated", get(associated))
        .route("/api/conversations/{id}/messages", get(list_msg).post(send_msg))
        .route("/api/conversations/{id}/messages/{messageId}", get(get_msg))
        .route("/api/conversations/{id}/ratings/{answerMessageId}", post(submit_rating))
        .route("/api/conversations/{id}/artifacts", get(list_artifacts))
        .route("/api/conversations/{id}/artifacts/{artifactId}", patch(update_artifact))
        .route("/api/conversations/{id}/cancel", post(cancel))
        .route("/api/conversations/{id}/runtime/ensure", post(ensure_runtime))
        .route("/api/conversations/{id}/active-lease", post(active_lease))
        // Confirmation system
        .route("/api/conversations/{id}/confirmations", get(list_confirmations))
        .route("/api/conversations/{id}/confirmations/{callId}/confirm", post(confirm))
        .route("/api/conversations/{id}/approvals/check", get(check_approval))
        .route("/api/conversations/active-count", get(active_count))
        .route("/api/conversations/clone", post(clone))
        .route("/api/messages/search", get(search_messages))
        .with_state(state)
}

#[derive(serde::Deserialize)]
struct RatingPathParams {
    id: String,
    #[serde(rename = "answerMessageId")]
    answer_message_id: String,
}

async fn submit_rating(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(params): Path<RatingPathParams>,
    body: Result<Json<SubmitConversationRatingRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ConversationRatingResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state.service.get(&user.id, &params.id).await.map_err(ApiError::from)?;

    match req.vote {
        ConversationRatingVote::Up if !(6..=10).contains(&req.score) => {
            return Err(ApiError::BadRequest("like score must be between 6 and 10".to_owned()));
        }
        ConversationRatingVote::Down if !(0..=5).contains(&req.score) => {
            return Err(ApiError::BadRequest("dislike score must be between 0 and 5".to_owned()));
        }
        _ => {}
    }

    let question = state
        .service
        .get_message(&user.id, &params.id, &req.question_message_id)
        .await
        .map_err(ApiError::from)?;
    let answer = state
        .service
        .get_message(&user.id, &params.id, &params.answer_message_id)
        .await
        .map_err(ApiError::from)?;
    if question.r#type != MessageType::Text || question.position != Some(MessagePosition::Right) {
        return Err(ApiError::BadRequest(
            "question_message_id must reference a right-side text message".to_owned(),
        ));
    }
    if answer.r#type != MessageType::Text || answer.position != Some(MessagePosition::Left) {
        return Err(ApiError::BadRequest(
            "answer_message_id must reference a left-side text message".to_owned(),
        ));
    }

    let vote = match req.vote {
        ConversationRatingVote::Up => "up",
        ConversationRatingVote::Down => "down",
    };
    let comment = req.comment.as_deref().map(str::trim).filter(|value| !value.is_empty());
    let id = format!("rating_{}", generate_short_id());
    let now = now_ms();
    let row = sqlx::query(
        "INSERT INTO conversation_ratings \
            (id, user_id, conversation_id, question_message_id, answer_message_id, vote, score, comment, \
             question_snapshot, answer_snapshot, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id, conversation_id, answer_message_id) DO UPDATE SET \
            question_message_id = excluded.question_message_id, vote = excluded.vote, score = excluded.score, \
            comment = excluded.comment, question_snapshot = excluded.question_snapshot, \
            answer_snapshot = excluded.answer_snapshot, updated_at = excluded.updated_at \
         RETURNING *",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&params.id)
    .bind(&req.question_message_id)
    .bind(&params.answer_message_id)
    .bind(vote)
    .bind(req.score)
    .bind(comment)
    .bind(&req.question_snapshot)
    .bind(&req.answer_snapshot)
    .bind(now)
    .bind(now)
    .fetch_one(&state.rating_pool)
    .await
    .map_err(|error| ApiError::Internal(format!("failed to save conversation rating: {error}")))?;

    let stored_vote: String = row
        .try_get("vote")
        .map_err(|error| ApiError::Internal(format!("invalid stored rating: {error}")))?;
    let vote = match stored_vote.as_str() {
        "up" => ConversationRatingVote::Up,
        "down" => ConversationRatingVote::Down,
        _ => return Err(ApiError::Internal("invalid stored rating vote".to_owned())),
    };
    Ok(Json(ApiResponse::ok(ConversationRatingResponse {
        id: row.try_get("id").map_err(rating_row_error)?,
        user_id: row.try_get("user_id").map_err(rating_row_error)?,
        conversation_id: row.try_get("conversation_id").map_err(rating_row_error)?,
        question_message_id: row.try_get("question_message_id").map_err(rating_row_error)?,
        answer_message_id: row.try_get("answer_message_id").map_err(rating_row_error)?,
        vote,
        score: row.try_get("score").map_err(rating_row_error)?,
        comment: row.try_get("comment").map_err(rating_row_error)?,
        question_snapshot: row.try_get("question_snapshot").map_err(rating_row_error)?,
        answer_snapshot: row.try_get("answer_snapshot").map_err(rating_row_error)?,
        created_at: row.try_get("created_at").map_err(rating_row_error)?,
        updated_at: row.try_get("updated_at").map_err(rating_row_error)?,
    })))
}

fn rating_row_error(error: sqlx::Error) -> ApiError {
    ApiError::Internal(format!("invalid stored rating: {error}"))
}

// ── Handlers ───────────────────────────────────────────────────────

async fn create(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    body: Result<Json<CreateConversationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ApiResponse<ConversationResponse>>), ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let conversation = state.service.create(&user.id, req).await.map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(conversation))))
}

async fn list(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<ListConversationsQuery>,
) -> Result<Json<ApiResponse<ConversationListResponse>>, ApiError> {
    let result = state.service.list(&user.id, query).await.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

async fn clone(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    body: Result<Json<CloneConversationRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ApiResponse<ConversationResponse>>), ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let conversation = state
        .service
        .clone_create(&user.id, req)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(ApiResponse::ok(conversation))))
}

async fn get_one(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ConversationResponse>>, ApiError> {
    let conversation = state.service.get(&user.id, &id).await.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(conversation)))
}

async fn update(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<UpdateConversationRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ConversationResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let conversation = state
        .service
        .update(&user.id, &id, req, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(conversation)))
}

async fn delete_one(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    state.service.delete(&user.id, &id).await.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::success()))
}

async fn reset(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    state.service.reset(&user.id, &id).await.map_err(ApiError::from)?;
    Ok(Json(ApiResponse::success()))
}

async fn associated(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<ConversationResponse>>>, ApiError> {
    let items = state
        .service
        .list_associated(&user.id, &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(items)))
}

async fn list_msg(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Query(query): Query<ListMessagesQuery>,
) -> Result<Json<ApiResponse<MessageListResponse>>, ApiError> {
    let result = state
        .service
        .list_messages(&user.id, &id, query)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

#[derive(serde::Deserialize)]
struct MessagePathParams {
    id: String,
    #[serde(rename = "messageId")]
    message_id: String,
}

async fn get_msg(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(params): Path<MessagePathParams>,
) -> Result<Json<ApiResponse<MessageResponse>>, ApiError> {
    let result = state
        .service
        .get_message(&user.id, &params.id, &params.message_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

async fn send_msg(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<SendMessageRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ApiResponse<SendMessageResponse>>), ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let response = state
        .service
        .send_message(&user.id, &id, req, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::ok(response))))
}

async fn list_artifacts(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ConversationArtifactListResponse>>, ApiError> {
    let result = state
        .service
        .list_artifacts(&user.id, &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

#[derive(serde::Deserialize)]
struct ArtifactPathParams {
    id: String,
    #[serde(rename = "artifactId")]
    artifact_id: String,
}

async fn update_artifact(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(params): Path<ArtifactPathParams>,
    body: Result<Json<UpdateConversationArtifactRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ConversationArtifactResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let artifact = state
        .service
        .update_artifact(&user.id, &params.id, &params.artifact_id, req)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(artifact)))
}

async fn cancel(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<CancelConversationRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<CancelConversationResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let response = state
        .service
        .cancel(&user.id, &id, &req.turn_id, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(response)))
}

async fn ensure_runtime(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<EnsureConversationRuntimeResponse>>, ApiError> {
    let response = state
        .service
        .ensure_runtime(&user.id, &id, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(response)))
}

async fn active_lease(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    state
        .service
        .renew_active_lease(&user.id, &id, &state.active_leases)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::success()))
}

async fn search_messages(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Query(query): Query<SearchMessagesQuery>,
) -> Result<Json<ApiResponse<MessageSearchResponse>>, ApiError> {
    let result = state
        .service
        .search_messages(&user.id, query)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

// ── Confirmation handlers ─────────────────────────────────────────

async fn list_confirmations(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ConfirmationListResponse>>, ApiError> {
    let items = state
        .service
        .list_confirmations(&user.id, &id, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(items)))
}

#[derive(serde::Deserialize)]
struct ConfirmPathParams {
    id: String,
    #[serde(rename = "callId")]
    call_id: String,
}

async fn confirm(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(params): Path<ConfirmPathParams>,
    body: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    state
        .service
        .confirm(&user.id, &params.id, &params.call_id, req, &state.task_manager)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::success()))
}

async fn check_approval(
    State(state): State<ConversationRouterState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Query(query): Query<ApprovalCheckQuery>,
) -> Result<Json<ApiResponse<ApprovalCheckResponse>>, ApiError> {
    if query.action.trim().is_empty() {
        return Err(ApiError::BadRequest("action must not be empty".into()));
    }

    let result = state
        .service
        .check_approval(
            &user.id,
            &id,
            &query.action,
            query.command_type.as_deref(),
            &state.task_manager,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ApiResponse::ok(result)))
}

async fn active_count(
    State(state): State<ConversationRouterState>,
    Extension(_user): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<ActiveCountResponse>>, ApiError> {
    let count = state.task_manager.active_count();
    Ok(Json(ApiResponse::ok(ActiveCountResponse { count })))
}

#[cfg(test)]
mod error_mapping_tests {
    use super::*;

    #[test]
    fn conversation_not_found_maps_to_app_not_found() {
        let app = ApiError::from(ConversationError::NotFound { id: "conv_1".into() });
        assert!(matches!(app, ApiError::NotFound(message) if message == "Conversation conv_1 not found"));
    }

    #[test]
    fn conversation_archived_maps_to_app_conversation_archived() {
        let app = ApiError::from(ConversationError::Archived {
            id: "conv_1".into(),
            reason: "legacy runtime".into(),
        });
        assert!(matches!(app, ApiError::ConversationArchived(message) if message == "legacy runtime"));
    }

    #[test]
    fn message_not_found_maps_to_app_not_found() {
        let app = ApiError::from(ConversationError::MessageNotFound { id: "msg_1".into() });
        assert!(matches!(app, ApiError::NotFound(message) if message == "Message msg_1 not found"));
    }

    #[test]
    fn artifact_not_found_maps_to_app_not_found() {
        let app = ApiError::from(ConversationError::ArtifactNotFound {
            id: "artifact_1".into(),
        });
        assert!(matches!(app, ApiError::NotFound(message) if message == "Artifact artifact_1 not found"));
    }

    #[test]
    fn active_agent_not_found_maps_to_app_not_found() {
        let app = ApiError::from(ConversationError::ActiveAgentNotFound {
            conversation_id: "conv_1".into(),
        });
        assert!(matches!(app, ApiError::NotFound(message) if message == "No active agent for this conversation"));
    }

    #[test]
    fn conversation_api_error_compat_preserves_special_codes() {
        let app = ApiError::from(ConversationError::WorkspacePathRuntimeUnavailable {
            path: "/tmp/my project".into(),
        });
        assert!(matches!(
            app,
            ApiError::WorkspacePathRuntimeUnavailable(message) if message == "/tmp/my project"
        ));
    }

    #[test]
    fn openclaw_gateway_unreachable_maps_to_coded_bad_gateway() {
        let app = ApiError::from(ConversationError::OpenClawGatewayUnreachable {
            detail: "OpenClaw Gateway is not running or cannot be reached at 127.0.0.1:18789.".into(),
        });

        assert_eq!(app.status_code(), StatusCode::BAD_GATEWAY);
        assert_eq!(app.error_code(), "USER_AGENT_OPENCLAW_GATEWAY_UNREACHABLE");
        assert_eq!(app.public_message(), "OpenClaw Gateway is not reachable");
        let details = app.error_details().expect("details should be present");
        assert_eq!(details["backend"], "openclaw");
        assert_eq!(details["port"], 18789);
    }
}
