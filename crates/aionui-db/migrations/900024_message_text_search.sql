-- Fork migration 900024: index user-visible text without scanning serialized tool payloads.
-- The FTS rowid mirrors messages.rowid so trigger maintenance and joins stay cheap.

CREATE VIRTUAL TABLE IF NOT EXISTS message_text_search USING fts5(
    content,
    tokenize = 'trigram'
);

DELETE FROM message_text_search;

INSERT INTO message_text_search(rowid, content)
SELECT
    m.rowid,
    CASE
        WHEN json_valid(m.content) THEN COALESCE(
            (
                SELECT group_concat(j.atom, ' ')
                FROM json_tree(m.content) AS j
                WHERE j.type = 'text'
            ),
            m.content
        )
        ELSE m.content
    END
FROM messages AS m
WHERE m.type = 'text'
  AND (m.status IS NULL OR m.status <> 'work');

CREATE TRIGGER IF NOT EXISTS trg_message_text_search_insert
AFTER INSERT ON messages
WHEN NEW.type = 'text'
  AND (NEW.status IS NULL OR NEW.status <> 'work')
BEGIN
    INSERT INTO message_text_search(rowid, content)
    VALUES (
        NEW.rowid,
        CASE
            WHEN json_valid(NEW.content) THEN COALESCE(
                (
                    SELECT group_concat(j.atom, ' ')
                    FROM json_tree(NEW.content) AS j
                    WHERE j.type = 'text'
                ),
                NEW.content
            )
            ELSE NEW.content
        END
    );
END;

CREATE TRIGGER IF NOT EXISTS trg_message_text_search_update
AFTER UPDATE OF type, content, status ON messages
WHEN (
    OLD.type = 'text'
    AND (OLD.status IS NULL OR OLD.status <> 'work')
) OR (
    NEW.type = 'text'
    AND (NEW.status IS NULL OR NEW.status <> 'work')
)
BEGIN
    DELETE FROM message_text_search WHERE rowid = OLD.rowid;
    INSERT INTO message_text_search(rowid, content)
    SELECT
        NEW.rowid,
        CASE
            WHEN json_valid(NEW.content) THEN COALESCE(
                (
                    SELECT group_concat(j.atom, ' ')
                    FROM json_tree(NEW.content) AS j
                    WHERE j.type = 'text'
                ),
                NEW.content
            )
            ELSE NEW.content
        END
    WHERE NEW.type = 'text'
      AND (NEW.status IS NULL OR NEW.status <> 'work');
END;

CREATE TRIGGER IF NOT EXISTS trg_message_text_search_delete
AFTER DELETE ON messages
WHEN OLD.type = 'text'
  AND (OLD.status IS NULL OR OLD.status <> 'work')
BEGIN
    DELETE FROM message_text_search WHERE rowid = OLD.rowid;
END;
