use crate::error::DbError;
use crate::models::{ConversationRatingRow, UpsertConversationRatingParams};

#[async_trait::async_trait]
pub trait IConversationRatingRepository: Send + Sync {
    async fn upsert(&self, params: &UpsertConversationRatingParams<'_>) -> Result<ConversationRatingRow, DbError>;

    async fn get_for_answer(
        &self,
        user_id: &str,
        conversation_id: &str,
        answer_message_id: &str,
    ) -> Result<Option<ConversationRatingRow>, DbError>;
}
