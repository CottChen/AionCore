use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Row mapping for the `conversation_ratings` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConversationRatingRow {
    pub id: String,
    pub user_id: String,
    pub conversation_id: String,
    pub question_message_id: String,
    pub answer_message_id: String,
    pub vote: String,
    pub score: i64,
    pub comment: Option<String>,
    pub question_snapshot: String,
    pub answer_snapshot: String,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
}

#[derive(Debug, Clone)]
pub struct UpsertConversationRatingParams<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub conversation_id: &'a str,
    pub question_message_id: &'a str,
    pub answer_message_id: &'a str,
    pub vote: &'a str,
    pub score: i64,
    pub comment: Option<&'a str>,
    pub question_snapshot: &'a str,
    pub answer_snapshot: &'a str,
    pub now: TimestampMs,
}
