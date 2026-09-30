//! Estimate the provider context-window request without mutating prompt content.
use crate::providers::ChatMessage;
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 8_192;
const MAX_CONTEXT_WINDOW_TOKENS: u32 = 32_768;
fn estimate_tokens(text: &str) -> u32 {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    (chars / 4).max(1)
}

pub(super) fn compute_dynamic_num_ctx(
    system_prompt: &str,
    messages: &[ChatMessage],
    model_context_window: Option<i32>,
    max_tokens: u32,
) -> ContextBudgetResult {
    let prompt_tokens = messages
        .iter()
        .fold(estimate_tokens(system_prompt), |total, message| {
            total.saturating_add(estimate_tokens(&message.content))
        });
    let needed = prompt_tokens
        .saturating_add(max_tokens)
        .saturating_add(1_024);
    let model_limit = model_context_window
        .and_then(|window| u32::try_from(window).ok())
        .filter(|window| *window > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
        .min(MAX_CONTEXT_WINDOW_TOKENS);

    let num_ctx = needed.clamp(DEFAULT_CONTEXT_WINDOW_TOKENS.min(model_limit), model_limit);
    ContextBudgetResult {
        num_ctx,
        needed,
        prompt_tokens,
        estimated_context_limit_exceeded: needed > model_limit,
    }
}

pub(super) struct ContextBudgetResult {
    pub num_ctx: u32,
    pub needed: u32,
    pub prompt_tokens: u32,
    pub estimated_context_limit_exceeded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_prompt_is_disclosed_but_never_claimed_trimmed() {
        for role in ["user", "assistant"] {
            let messages = vec![ChatMessage {
                role: role.into(),
                content: "x".repeat(100_000),
                images: None,
            }];
            let before = messages[0].content.clone();
            let budget = compute_dynamic_num_ctx("custom goal", &messages, Some(8192), 2048);
            assert!(budget.estimated_context_limit_exceeded);
            assert_eq!(budget.num_ctx, 8192);
            assert_eq!(messages[0].content, before);
        }
        let budget = compute_dynamic_num_ctx(&"x".repeat(100_000), &[], Some(8192), 2048);
        assert!(budget.estimated_context_limit_exceeded);
        assert!(
            !compute_dynamic_num_ctx("normal", &[], Some(8192), 2048)
                .estimated_context_limit_exceeded
        );
    }
}
