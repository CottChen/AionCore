use aionui_db::MessagePageCursor;
use base64::Engine;

use crate::ConversationError;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct MessageCursorV1 {
    created_at: i64,
    id: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct MessageSearchCursorV1 {
    created_at: i64,
    id: String,
    keyword: String,
    user_id: String,
    /// Cursors issued before this field existed decode as `false`, i.e. whole-transcript search.
    #[serde(default)]
    user_only: bool,
}

pub fn encode_message_cursor(cursor: &MessagePageCursor) -> Result<String, ConversationError> {
    let json = serde_json::to_vec(&MessageCursorV1 {
        created_at: cursor.created_at,
        id: cursor.id.clone(),
    })
    .map_err(|e| ConversationError::internal(format!("Failed to encode message cursor: {e}")))?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
    Ok(format!("v1.{encoded}"))
}

pub fn decode_message_cursor(raw: &str) -> Result<MessagePageCursor, ConversationError> {
    let encoded = raw
        .strip_prefix("v1.")
        .ok_or_else(|| ConversationError::bad_request("invalid message cursor"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| ConversationError::bad_request("invalid message cursor"))?;
    let cursor: MessageCursorV1 =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::bad_request("invalid message cursor"))?;
    if cursor.id.is_empty() {
        return Err(ConversationError::bad_request("invalid message cursor"));
    }
    Ok(MessagePageCursor {
        created_at: cursor.created_at,
        id: cursor.id,
    })
}

/// Encodes a search cursor together with the normalized query it belongs to.
/// Keeping the query in the opaque token prevents a cursor from being reused
/// after the user changes the search keyword or the search scope.
pub fn encode_search_message_cursor(
    cursor: &MessagePageCursor,
    keyword: &str,
    user_id: &str,
    user_only: bool,
) -> Result<String, ConversationError> {
    let json = serde_json::to_vec(&MessageSearchCursorV1 {
        created_at: cursor.created_at,
        id: cursor.id.clone(),
        keyword: keyword.trim().to_string(),
        user_id: user_id.to_string(),
        user_only,
    })
    .map_err(|e| ConversationError::internal(format!("Failed to encode message search cursor: {e}")))?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
    Ok(format!("v1s.{encoded}"))
}

pub fn decode_search_message_cursor(
    raw: &str,
    keyword: &str,
    user_id: &str,
    user_only: bool,
) -> Result<MessagePageCursor, ConversationError> {
    let encoded = raw
        .strip_prefix("v1s.")
        .ok_or_else(|| ConversationError::bad_request("invalid message search cursor"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| ConversationError::bad_request("invalid message search cursor"))?;
    let cursor: MessageSearchCursorV1 =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::bad_request("invalid message search cursor"))?;
    if cursor.id.is_empty()
        || cursor.keyword != keyword.trim()
        || cursor.user_id != user_id
        || cursor.user_only != user_only
    {
        return Err(ConversationError::bad_request("invalid message search cursor"));
    }
    Ok(MessagePageCursor {
        created_at: cursor.created_at,
        id: cursor.id,
    })
}

#[cfg(test)]
mod tests {
    use aionui_db::MessagePageCursor;

    use super::*;

    #[test]
    fn message_cursor_round_trips_created_at_and_id() {
        let key = MessagePageCursor {
            created_at: 1234,
            id: "msg-b".to_string(),
        };

        let cursor = encode_message_cursor(&key).unwrap();

        assert_eq!(decode_message_cursor(&cursor).unwrap(), key);
    }

    #[test]
    fn message_cursor_rejects_invalid_shapes() {
        for raw in ["", "v2.abc", "v1.%%%%", "v1.e30", "v1.eyJjcmVhdGVkX2F0IjoxLCJpZCI6IiJ9"] {
            let err = decode_message_cursor(raw).unwrap_err();
            assert!(matches!(err, ConversationError::BadRequest { reason } if reason == "invalid message cursor"));
        }
    }

    #[test]
    fn search_cursor_is_bound_to_keyword() {
        let key = MessagePageCursor {
            created_at: 1234,
            id: "msg-b".to_string(),
        };
        let cursor = encode_search_message_cursor(&key, "  keyword ", "user-1", false).unwrap();

        assert_eq!(
            decode_search_message_cursor(&cursor, "keyword", "user-1", false).unwrap(),
            key
        );
        assert!(matches!(
            decode_search_message_cursor(&cursor, "other", "user-1", false),
            Err(ConversationError::BadRequest { reason }) if reason == "invalid message search cursor"
        ));
        assert!(matches!(
            decode_search_message_cursor(&cursor, "keyword", "user-2", false),
            Err(ConversationError::BadRequest { reason }) if reason == "invalid message search cursor"
        ));
    }

    #[test]
    fn search_cursor_is_bound_to_scope() {
        let key = MessagePageCursor {
            created_at: 1234,
            id: "msg-b".to_string(),
        };

        // A cursor minted for the whole transcript must not continue a user-only search.
        let all_scope = encode_search_message_cursor(&key, "keyword", "user-1", false).unwrap();
        assert_eq!(
            decode_search_message_cursor(&all_scope, "keyword", "user-1", false).unwrap(),
            key
        );
        assert!(matches!(
            decode_search_message_cursor(&all_scope, "keyword", "user-1", true),
            Err(ConversationError::BadRequest { reason }) if reason == "invalid message search cursor"
        ));

        let user_scope = encode_search_message_cursor(&key, "keyword", "user-1", true).unwrap();
        assert_eq!(
            decode_search_message_cursor(&user_scope, "keyword", "user-1", true).unwrap(),
            key
        );
        assert!(matches!(
            decode_search_message_cursor(&user_scope, "keyword", "user-1", false),
            Err(ConversationError::BadRequest { reason }) if reason == "invalid message search cursor"
        ));
    }

    #[test]
    fn search_cursor_without_scope_decodes_as_whole_transcript() {
        // Cursors issued before the scope field existed stay usable for default searches.
        let legacy = format!(
            "v1s.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::json!({
                    "created_at": 1234,
                    "id": "msg-b",
                    "keyword": "keyword",
                    "user_id": "user-1",
                })
                .to_string()
            )
        );

        assert_eq!(
            decode_search_message_cursor(&legacy, "keyword", "user-1", false).unwrap(),
            MessagePageCursor {
                created_at: 1234,
                id: "msg-b".to_string(),
            }
        );
        assert!(matches!(
            decode_search_message_cursor(&legacy, "keyword", "user-1", true),
            Err(ConversationError::BadRequest { reason }) if reason == "invalid message search cursor"
        ));
    }
}
