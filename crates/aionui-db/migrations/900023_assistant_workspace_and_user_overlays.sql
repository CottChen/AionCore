ALTER TABLE assistant_definitions
    ADD COLUMN default_workspace_mode TEXT NOT NULL DEFAULT 'auto'
        CHECK (default_workspace_mode IN ('auto', 'fixed'));

ALTER TABLE assistant_definitions
    ADD COLUMN default_workspace_value TEXT;

CREATE TABLE IF NOT EXISTS assistant_user_overlays (
    user_id                 TEXT    NOT NULL,
    assistant_definition_id TEXT    NOT NULL,
    enabled                 INTEGER,
    sort_order              INTEGER,
    agent_id_override       TEXT,
    last_used_at            INTEGER,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    PRIMARY KEY (user_id, assistant_definition_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (assistant_definition_id) REFERENCES assistant_definitions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_assistant_user_overlays_user
    ON assistant_user_overlays(user_id);

CREATE INDEX IF NOT EXISTS idx_assistant_user_overlays_sort_order
    ON assistant_user_overlays(user_id, sort_order);

CREATE TABLE IF NOT EXISTS user_client_preferences (
    user_id    TEXT    NOT NULL,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_user_client_preferences_user
    ON user_client_preferences(user_id);
