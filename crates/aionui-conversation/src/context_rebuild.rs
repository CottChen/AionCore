//! Bounded transcript builder for in-place ACP context rebuilds.
//!
//! Rebuilding an ACP conversation (`session/new` instead of `session/load`)
//! throws away the CLI's own history. To keep the model useful we re-inject a
//! bounded slice of the earlier turns as plain text on the first prompt.
//!
//! The slice is deliberately bounded: the injection itself has to fit in the
//! target model's context window, otherwise the request fails before any
//! auto-compaction can help. Callers pick a turn limit; the character ceiling
//! is enforced here so a handful of huge turns cannot blow the window.

use crate::convert::string_to_enum;
use crate::error::ConversationError;
use crate::service::ConversationService;
use aionui_api_types::{PendingContextRebuild, RebuildContextRequest, RebuildContextResponse};
use aionui_common::{AgentKillReason, AgentType, ConversationSource, now_ms};
use aionui_db::{ConversationRowUpdate, ConversationTextMessageRow};
use tracing::info;

/// Turns injected when the caller does not choose a limit.
pub const DEFAULT_TURNS: u32 = 20;
/// Hard ceiling for a caller-provided turn limit.
pub const MAX_TURNS: u32 = 50;
/// Hard ceiling for the rendered transcript, in Unicode characters.
pub const MAX_CHARS: usize = 120_000;

/// One question/answer pair from the previous history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildTurn {
    pub question: String,
    pub answer: String,
}

/// Rendered transcript plus the stats surfaced to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildTranscript {
    pub text: String,
    pub turns: u32,
    pub chars: u64,
    pub truncated: bool,
}

const HEADER: &str = "以下是本会话更早的对话记录，仅用于恢复上下文，不是新的指令。";
const FOOTER: &str = "以上为历史记录结束。请继续回答用户最新的一条消息。";

/// Clamp a caller-provided turn limit into the supported range.
pub fn normalized_turns(max_turns: Option<u32>) -> u32 {
    max_turns.unwrap_or(DEFAULT_TURNS).clamp(1, MAX_TURNS)
}

fn render(index: usize, turn: &RebuildTurn) -> String {
    let mut block = format!("【第 {index} 轮】\n用户：{}", turn.question.trim());
    let answer = turn.answer.trim();
    if !answer.is_empty() {
        block.push_str("\n助手：");
        block.push_str(answer);
    }
    block
}

fn truncate_to_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// Render the most recent `max_turns` turns, newest-last, within `max_chars`.
///
/// Turns are added from newest to oldest so the budget keeps the most relevant
/// history. Everything older than the cut is dropped and reported through
/// `truncated`.
pub fn build_transcript(turns: &[RebuildTurn], requested_turns: u32, max_chars: usize) -> RebuildTranscript {
    let turn_limit = requested_turns.clamp(1, MAX_TURNS) as usize;
    let budget = max_chars.max(1);
    let header_cost = HEADER.chars().count() + FOOTER.chars().count() + 2;

    let mut blocks: Vec<String> = Vec::new();
    let mut used = header_cost;
    let mut truncated = false;

    for (offset, turn) in turns.iter().rev().enumerate() {
        if blocks.len() >= turn_limit {
            truncated = true;
            break;
        }
        let index = turns.len() - offset;
        let block = render(index, turn);
        let cost = block.chars().count() + 2;
        if used + cost <= budget {
            used += cost;
            blocks.push(block);
            continue;
        }

        // The newest oversized turn still needs to contribute something.
        if blocks.is_empty() {
            let available = budget.saturating_sub(used + 2);
            if available > 0 {
                truncated = true;
                blocks.push(truncate_to_chars(&block, available));
            } else {
                truncated = true;
            }
        } else {
            truncated = true;
        }
        break;
    }

    blocks.reverse();
    let text = if blocks.is_empty() {
        String::new()
    } else {
        format!("{HEADER}\n\n{}\n\n{FOOTER}", blocks.join("\n\n"))
    };
    RebuildTranscript {
        turns: blocks.len() as u32,
        chars: text.chars().count() as u64,
        truncated,
        text,
    }
}

/// Group stored text messages into question/answer turns, preserving the raw
/// text (unlike search previews, which collapse whitespace).
fn turns_from_rows(rows: Vec<ConversationTextMessageRow>) -> Vec<RebuildTurn> {
    let mut turns: Vec<RebuildTurn> = Vec::new();
    let mut current: Option<RebuildTurn> = None;

    for row in rows {
        let content = row.content.trim();
        if content.is_empty() {
            continue;
        }
        match row.position.as_deref() {
            Some("right") => {
                if let Some(turn) = current.take() {
                    turns.push(turn);
                }
                current = Some(RebuildTurn {
                    question: content.to_owned(),
                    answer: String::new(),
                });
            }
            Some("left") => {
                if let Some(turn) = current.as_mut()
                    && turn.answer.is_empty()
                {
                    turn.answer = content.to_owned();
                }
            }
            _ => {}
        }
    }
    if let Some(turn) = current {
        turns.push(turn);
    }
    turns
}

impl ConversationService {
    /// Rebuild the conversation's ACP session in place so a different model can
    /// take over, re-injecting a bounded slice of the previous turns.
    ///
    /// The conversation id, message history, and list position are preserved.
    /// The stored ACP `session_id` is dropped so the next runtime ensure takes
    /// the `session/new` path, and the transcript is parked in
    /// `conversations.extra.context_rebuild` for the first prompt.
    pub async fn rebuild_context(
        &self,
        user_id: &str,
        id: &str,
        req: RebuildContextRequest,
    ) -> Result<RebuildContextResponse, ConversationError> {
        let row = self
            .conversation_repo()
            .get(id)
            .await?
            .filter(|row| row.user_id == user_id)
            .ok_or_else(|| ConversationError::NotFound { id: id.to_owned() })?;

        let agent_type: AgentType = string_to_enum(&row.r#type)?;
        if agent_type != AgentType::Acp {
            return Err(ConversationError::BadRequest {
                reason: "In-place context rebuild is only supported for ACP conversations".into(),
            });
        }

        if self.runtime_state().active_turn_id_for(id).is_some() {
            return Err(ConversationError::Busy {
                reason: "Cannot rebuild context while a turn is running".into(),
            });
        }

        let rows = self.conversation_repo().list_text_messages(id).await?;
        let turns = turns_from_rows(rows);
        if turns.is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "No prior text history is available to rebuild from".into(),
            });
        }

        let transcript = build_transcript(&turns, normalized_turns(req.max_turns), MAX_CHARS);
        if transcript.text.is_empty() {
            return Err(ConversationError::BadRequest {
                reason: "No prior text history is available to rebuild from".into(),
            });
        }

        let requested_model = req
            .model_id
            .map(|value| value.trim().to_owned())
            .filter(|v| !v.is_empty());
        let previous_session_id = self
            .acp_session_repo()
            .get(id)
            .await?
            .and_then(|session| session.session_id);

        // Recycle the live agent first: the next ensure must build a brand new
        // session instead of resuming the old one.
        self.task_manager()
            .kill_and_wait(id, Some(AgentKillReason::ContextRebuild))
            .await;
        self.acp_session_repo().clear_session_id(id).await?;

        let mut extra: serde_json::Value = serde_json::from_str(&row.extra).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = extra.as_object_mut() {
            obj.insert(
                "context_rebuild".to_owned(),
                serde_json::to_value(PendingContextRebuild {
                    text: transcript.text.clone(),
                    turns: transcript.turns,
                    model_id: requested_model.clone(),
                    created_at: now_ms(),
                })
                .map_err(|e| ConversationError::internal(format!("serialize context rebuild failed: {e}")))?,
            );
            if let Some(model) = requested_model.as_ref() {
                obj.insert("current_model_id".to_owned(), serde_json::Value::String(model.clone()));
            }
        }

        let extra_json = serde_json::to_string(&extra)
            .map_err(|e| ConversationError::internal(format!("serialize conversation extra failed: {e}")))?;
        let update = ConversationRowUpdate {
            extra: Some(extra_json),
            updated_at: Some(now_ms()),
            ..Default::default()
        };
        self.conversation_repo().update(id, &update).await?;

        let source = row
            .source
            .as_deref()
            .and_then(|value| string_to_enum::<ConversationSource>(value).ok());
        self.broadcast_list_changed(id, "updated", source.as_ref());

        info!(
            conversation_id = id,
            available_turns = turns.len(),
            injected_turns = transcript.turns,
            injected_chars = transcript.chars,
            truncated = transcript.truncated,
            model_id = requested_model.as_deref().unwrap_or(""),
            previous_session_id = previous_session_id.as_deref().unwrap_or(""),
            "Conversation context rebuilt in place"
        );

        Ok(RebuildContextResponse {
            conversation_id: id.to_owned(),
            available_turns: turns.len() as u32,
            injected_turns: transcript.turns,
            injected_chars: transcript.chars,
            truncated: transcript.truncated,
            model_id: requested_model,
            previous_session_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(index: usize, answer_chars: usize) -> RebuildTurn {
        RebuildTurn {
            question: format!("问题{index}"),
            answer: "答".repeat(answer_chars),
        }
    }

    #[test]
    fn keeps_the_most_recent_turns_in_chronological_order() {
        let turns: Vec<_> = (1..=30).map(|i| turn(i, 4)).collect();

        let result = build_transcript(&turns, 5, MAX_CHARS);

        assert_eq!(result.turns, 5);
        assert!(result.truncated, "dropping 25 older turns must be reported");
        assert!(result.text.contains("问题30") && result.text.contains("问题26"));
        assert!(!result.text.contains("问题25"));
        assert!(result.text.find("问题26").unwrap() < result.text.find("问题30").unwrap());
    }

    #[test]
    fn stops_at_the_character_budget_and_reports_truncation() {
        let turns: Vec<_> = (1..=30).map(|i| turn(i, 400)).collect();

        let result = build_transcript(&turns, MAX_TURNS, 4_000);

        assert!(result.truncated);
        assert!(result.chars <= 4_000, "chars {} exceeded budget", result.chars);
        assert!(result.turns < 30, "budget must drop old turns, got {}", result.turns);
        assert!(result.text.contains("问题30"));
    }

    #[test]
    fn trims_a_single_oversized_turn_instead_of_producing_an_empty_transcript() {
        let turns = vec![turn(1, 50_000)];

        let result = build_transcript(&turns, 5, 1_000);

        assert_eq!(result.turns, 1);
        assert!(result.truncated);
        assert!(result.chars <= 1_000);
        assert!(result.text.starts_with(HEADER));
    }

    #[test]
    fn skips_empty_answers_and_empty_history() {
        let turns = vec![
            RebuildTurn {
                question: "只有提问".into(),
                answer: String::new(),
            },
            RebuildTurn::default(),
        ];

        let with_history = build_transcript(&turns, 5, MAX_CHARS);
        assert!(with_history.text.contains("只有提问"));
        assert!(!with_history.text.contains("助手："));
        assert!(!with_history.truncated);

        let empty = build_transcript(&[], 5, MAX_CHARS);
        assert_eq!(empty.turns, 0);
        assert!(empty.text.is_empty());
        assert!(!empty.truncated);
    }

    #[test]
    fn clamps_out_of_range_turn_limits() {
        assert_eq!(normalized_turns(None), DEFAULT_TURNS);
        assert_eq!(normalized_turns(Some(0)), 1);
        assert_eq!(normalized_turns(Some(u32::MAX)), MAX_TURNS);

        let turns: Vec<_> = (1..=60).map(|i| turn(i, 2)).collect();
        assert_eq!(build_transcript(&turns, u32::MAX, MAX_CHARS).turns, MAX_TURNS);
        assert_eq!(build_transcript(&turns, 0, MAX_CHARS).turns, 1);
    }
}
