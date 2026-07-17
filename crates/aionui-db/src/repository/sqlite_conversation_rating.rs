use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::{ConversationRatingRow, UpsertConversationRatingParams};
use crate::repository::conversation_rating::IConversationRatingRepository;

#[derive(Clone, Debug)]
pub struct SqliteConversationRatingRepository {
    pool: SqlitePool,
}

impl SqliteConversationRatingRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IConversationRatingRepository for SqliteConversationRatingRepository {
    async fn upsert(&self, params: &UpsertConversationRatingParams<'_>) -> Result<ConversationRatingRow, DbError> {
        let row = sqlx::query_as::<_, ConversationRatingRow>(
            "INSERT INTO conversation_ratings \
                (id, user_id, conversation_id, question_message_id, answer_message_id, vote, score, \
                 comment, question_snapshot, answer_snapshot, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, conversation_id, answer_message_id) DO UPDATE SET \
                question_message_id = excluded.question_message_id, \
                vote = excluded.vote, \
                score = excluded.score, \
                comment = excluded.comment, \
                question_snapshot = excluded.question_snapshot, \
                answer_snapshot = excluded.answer_snapshot, \
                updated_at = excluded.updated_at \
             RETURNING *",
        )
        .bind(params.id)
        .bind(params.user_id)
        .bind(params.conversation_id)
        .bind(params.question_message_id)
        .bind(params.answer_message_id)
        .bind(params.vote)
        .bind(params.score)
        .bind(params.comment)
        .bind(params.question_snapshot)
        .bind(params.answer_snapshot)
        .bind(params.now)
        .bind(params.now)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }

    async fn get_for_answer(
        &self,
        user_id: &str,
        conversation_id: &str,
        answer_message_id: &str,
    ) -> Result<Option<ConversationRatingRow>, DbError> {
        let row = sqlx::query_as::<_, ConversationRatingRow>(
            "SELECT * FROM conversation_ratings \
             WHERE user_id = ? AND conversation_id = ? AND answer_message_id = ?",
        )
        .bind(user_id)
        .bind(conversation_id)
        .bind(answer_message_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    async fn setup() -> SqliteConversationRatingRepository {
        let db = init_database_memory().await.unwrap();
        let pool = db.pool().clone();
        let now = aionui_common::now_ms();
        sqlx::query(
            "INSERT OR IGNORE INTO users (id, username, password_hash, created_at, updated_at) \
             VALUES ('user_rating_test', 'rating-test', 'hash', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO conversations \
                (id, user_id, name, type, extra, model, status, source, channel_chat_id, pinned, pinned_at, created_at, updated_at) \
             VALUES ('conv_rating_test', 'user_rating_test', 'Rating Test', 'acp', '{}', NULL, 'pending', 'aionui', NULL, 0, NULL, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        SqliteConversationRatingRepository::new(pool)
    }

    #[tokio::test]
    async fn upsert_updates_existing_answer_rating() {
        let repo = setup().await;
        let now = aionui_common::now_ms();
        let first = repo
            .upsert(&UpsertConversationRatingParams {
                id: "rating_first",
                user_id: "user_rating_test",
                conversation_id: "conv_rating_test",
                question_message_id: "question_1",
                answer_message_id: "answer_1",
                vote: "up",
                score: 8,
                comment: Some("good"),
                question_snapshot: "question",
                answer_snapshot: "answer",
                now,
            })
            .await
            .unwrap();
        assert_eq!(first.id, "rating_first");
        assert_eq!(first.score, 8);

        let updated = repo
            .upsert(&UpsertConversationRatingParams {
                id: "rating_second",
                user_id: "user_rating_test",
                conversation_id: "conv_rating_test",
                question_message_id: "question_1",
                answer_message_id: "answer_1",
                vote: "down",
                score: 4,
                comment: Some("needs work"),
                question_snapshot: "question updated",
                answer_snapshot: "answer updated",
                now: now + 1,
            })
            .await
            .unwrap();

        assert_eq!(updated.id, "rating_first");
        assert_eq!(updated.vote, "down");
        assert_eq!(updated.score, 4);
        assert_eq!(updated.comment.as_deref(), Some("needs work"));
        assert_eq!(updated.question_snapshot, "question updated");
        assert_eq!(updated.answer_snapshot, "answer updated");
        assert_eq!(updated.created_at, now);
        assert_eq!(updated.updated_at, now + 1);

        let loaded = repo
            .get_for_answer("user_rating_test", "conv_rating_test", "answer_1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, updated);
    }
}
