CREATE TABLE IF NOT EXISTS conversation_ratings (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    question_message_id TEXT NOT NULL,
    answer_message_id TEXT NOT NULL,
    vote TEXT NOT NULL CHECK (vote IN ('up', 'down')),
    score INTEGER NOT NULL CHECK (score >= 0 AND score <= 10),
    comment TEXT,
    question_snapshot TEXT NOT NULL,
    answer_snapshot TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    UNIQUE(user_id, conversation_id, answer_message_id)
);

CREATE INDEX IF NOT EXISTS idx_conversation_ratings_conversation
    ON conversation_ratings(conversation_id, answer_message_id);

CREATE INDEX IF NOT EXISTS idx_conversation_ratings_user
    ON conversation_ratings(user_id, updated_at);
