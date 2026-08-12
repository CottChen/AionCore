pub(crate) const MARKDOWN_LINK_TARGET_RULE: &str = "When a Markdown link target contains spaces, Chinese parentheses, or other characters that common Markdown parsers may truncate, wrap the target in `<...>`.";

pub(crate) fn append_default_prompt_rules(base: Option<&str>) -> Option<String> {
    match base.map(str::trim).filter(|s| !s.is_empty()) {
        Some(base) => Some(format!("{base}\n\n{MARKDOWN_LINK_TARGET_RULE}")),
        None => Some(MARKDOWN_LINK_TARGET_RULE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_default_prompt_rules_adds_rule_to_existing_prompt() {
        assert_eq!(
            append_default_prompt_rules(Some("Be concise.")).as_deref(),
            Some(concat!(
                "Be concise.\n\n",
                "When a Markdown link target contains spaces, Chinese parentheses, or other characters that common Markdown parsers may truncate, wrap the target in `<...>`."
            ))
        );
    }

    #[test]
    fn append_default_prompt_rules_supplies_rule_when_base_missing() {
        assert_eq!(
            append_default_prompt_rules(None).as_deref(),
            Some(MARKDOWN_LINK_TARGET_RULE)
        );
        assert_eq!(
            append_default_prompt_rules(Some("  ")).as_deref(),
            Some(MARKDOWN_LINK_TARGET_RULE)
        );
    }
}
