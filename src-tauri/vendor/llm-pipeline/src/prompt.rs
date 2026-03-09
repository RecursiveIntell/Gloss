use crate::types::PipelineContext;

/// Sentinel that should never appear in real templates.
const ESCAPE_SENTINEL: &str = "\x00LBRACE\x00";
/// Sentinel for escaped closing brace.
const ESCAPE_SENTINEL_CLOSE: &str = "\x00RBRACE\x00";

/// Build a prompt string with variable substitution.
///
/// Replaces `{key}` placeholders in the template with values from the context.
/// The special `{input}` placeholder is replaced by the `input` parameter.
///
/// Use `{{` to insert a literal `{` and `}}` to insert a literal `}`.
///
/// # Example
///
/// ```
/// use llm_pipeline::types::PipelineContext;
/// use llm_pipeline::prompt::render;
///
/// let ctx = PipelineContext::new().insert("name", "Alice");
/// let result = render("Hello {name}, here is JSON: {{\"key\": \"val\"}}", "data", &ctx);
/// assert_eq!(result, r#"Hello Alice, here is JSON: {"key": "val"}"#);
/// ```
pub fn render(template: &str, input: &str, context: &PipelineContext) -> String {
    // Pass 1: protect escaped braces
    let mut rendered = template.replace("{{", ESCAPE_SENTINEL);
    rendered = rendered.replace("}}", ESCAPE_SENTINEL_CLOSE);

    // Pass 2: substitute placeholders
    rendered = rendered.replace("{input}", input);
    for (key, value) in &context.data {
        let placeholder = format!("{{{}}}", key);
        rendered = rendered.replace(&placeholder, value);
    }

    // Pass 3: restore escaped braces
    rendered = rendered.replace(ESCAPE_SENTINEL, "{");
    rendered = rendered.replace(ESCAPE_SENTINEL_CLOSE, "}");
    rendered
}

/// Create a numbered list from items (1-indexed).
pub fn numbered_list(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| format!("{}. {}", i + 1, item))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap text in a labeled section for structured prompts.
pub fn section(label: &str, content: &str) -> String {
    format!("## {}\n{}", label, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic() {
        let ctx = PipelineContext::new().insert("name", "Alice");
        let result = render("Hello {name}, process {input}", "data", &ctx);
        assert_eq!(result, "Hello Alice, process data");
    }

    #[test]
    fn test_render_no_placeholders() {
        let ctx = PipelineContext::new();
        let result = render("static prompt", "ignored_in_template", &ctx);
        assert_eq!(result, "static prompt");
    }

    #[test]
    fn test_numbered_list() {
        let items = vec![
            "First".to_string(),
            "Second".to_string(),
            "Third".to_string(),
        ];
        let result = numbered_list(&items);
        assert_eq!(result, "1. First\n2. Second\n3. Third");
    }

    #[test]
    fn test_numbered_list_empty() {
        let result = numbered_list(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_section() {
        let result = section("Context", "Some knowledge here");
        assert_eq!(result, "## Context\nSome knowledge here");
    }

    #[test]
    fn test_render_escaped_braces() {
        let ctx = PipelineContext::new().insert("name", "Alice");
        let result = render("Hello {name}, JSON: {{\"key\": \"val\"}}", "data", &ctx);
        assert_eq!(result, r#"Hello Alice, JSON: {"key": "val"}"#);
    }

    #[test]
    fn test_render_escaped_braces_no_substitution() {
        let ctx = PipelineContext::new();
        let result = render(
            "Output format: {{\"result\": {{\"value\": 42}}}}",
            "data",
            &ctx,
        );
        assert_eq!(result, r#"Output format: {"result": {"value": 42}}"#);
    }

    #[test]
    fn test_render_mixed_escaped_and_placeholder() {
        let ctx = PipelineContext::new().insert("schema", "string");
        let result = render(
            "Type is {schema}, format: {{\"type\": \"object\"}}",
            "x",
            &ctx,
        );
        assert_eq!(result, r#"Type is string, format: {"type": "object"}"#);
    }
}
