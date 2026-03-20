use crate::db::notebook_db::{Message, Source};
use crate::retrieval::hybrid_search::SearchResult;
use crate::retrieval::source_scope::ResolvedSourceScopeKind;

/// Maximum estimated tokens for the source manifest before switching to compact mode.
const MANIFEST_MAX_TOKENS: usize = 2000;
/// Maximum number of sources to include in the manifest.
/// Beyond this, the manifest notes how many were omitted.
const MANIFEST_MAX_SOURCES: usize = 50;

/// Assemble the context for an LLM chat request.
pub struct ContextAssembler;

#[derive(Debug, Clone)]
pub struct ContextPassage {
    pub source_id: String,
    pub title: String,
    pub content: String,
}

impl ContextAssembler {
    /// Build the system prompt. Source context is appended when present so it
    /// lives in the system message — not in user messages — which keeps history
    /// clean across turns.
    ///
    /// `manifest_sources` is the source list visible to the model for this turn.
    /// `source_context` is the retrieved content for grounding.
    pub fn build_system_prompt(
        custom_goal: Option<&str>,
        style: &str,
        scope_kind: ResolvedSourceScopeKind,
        manifest_sources: &[Source],
        source_context: &[ContextPassage],
    ) -> String {
        let mut prompt = String::new();

        // Style prefix
        match style {
            "learning_guide" => {
                prompt.push_str(
                    "You are a patient tutor. Before answering, ask clarifying questions to understand \
                     the user's learning goal and current knowledge level. Break complex topics into \
                     steps. Use analogies. After explaining, check understanding with a quick question.\n\n"
                );
            }
            "custom" => {
                // Custom style uses the custom_goal as the primary instruction
            }
            _ => {
                prompt.push_str(
                    "You are a knowledgeable research assistant helping the user understand their documents.\n\n"
                );
            }
        }

        // Custom goal
        if let Some(goal) = custom_goal {
            if !goal.is_empty() {
                prompt.push_str(&format!("Notebook goal: {}\n\n", goal));
            }
        }

        if matches!(
            scope_kind,
            ResolvedSourceScopeKind::All | ResolvedSourceScopeKind::Explicit
        ) && !manifest_sources.is_empty()
        {
            prompt.push_str(&Self::build_source_manifest(scope_kind, manifest_sources));
            prompt.push('\n');
        }

        // Selected source content + citation instructions
        if !source_context.is_empty() {
            prompt.push_str("Retrieved passages:\n\n");
            for (i, passage) in source_context.iter().enumerate() {
                prompt.push_str(&format!(
                    "[{}] {}\n{}\n\n",
                    i + 1,
                    passage.title,
                    passage.content
                ));
            }
            prompt.push_str(
                "When answering, cite the sources above using [1], [2], etc. \
                 Only cite information directly supported by these sources. \
                 Be concise.\n",
            );
        } else {
            match scope_kind {
                ResolvedSourceScopeKind::All if !manifest_sources.is_empty() => {
                    prompt.push_str(
                        "No specific passages were retrieved for this query, but you have access \
                         to the sources listed above. Use the source titles and summaries to provide \
                         a helpful response. If you cannot answer from the source information available, \
                         say so honestly and suggest the user refine their question.\n",
                    );
                }
                ResolvedSourceScopeKind::Explicit if !manifest_sources.is_empty() => {
                    prompt.push_str(
                        "No passages were retrieved from the selected sources for this query. \
                         Do not infer document details from source titles or summaries alone. \
                         Tell the user the current selection did not surface supporting passages \
                         and suggest refining the question or broadening the scope.\n",
                    );
                }
                ResolvedSourceScopeKind::None => {
                    prompt.push_str(
                        "No notebook sources are selected for this chat turn. \
                         Tell the user that no sources are currently selected and ask them to \
                         select sources or widen scope before relying on notebook content.\n",
                    );
                }
                _ => {}
            }
        }

        prompt
    }

    /// Build a compact source manifest for the effective chat scope.
    /// Uses compact mode (titles only) when the full manifest would exceed
    /// the token budget.
    fn build_source_manifest(scope_kind: ResolvedSourceScopeKind, sources: &[Source]) -> String {
        let total_count = sources.len();
        let capped = total_count > MANIFEST_MAX_SOURCES;
        let display_sources = if capped {
            &sources[..MANIFEST_MAX_SOURCES]
        } else {
            sources
        };

        // ~30 tokens per source with summary, ~10 without
        let estimated_full_tokens = display_sources.len() * 30;

        let header = format!(
            "{}{}:\n",
            match scope_kind {
                ResolvedSourceScopeKind::All => {
                    format!(
                        "You have access to {} sources in this notebook",
                        total_count
                    )
                }
                ResolvedSourceScopeKind::Explicit => {
                    format!("This chat is scoped to {} selected sources", total_count)
                }
                ResolvedSourceScopeKind::None => "No notebook sources are available".to_string(),
            },
            if capped {
                format!(" (showing {} of {})", MANIFEST_MAX_SOURCES, total_count)
            } else {
                String::new()
            },
        );

        let capped_footer = if capped {
            format!(
                "\n... and {} more sources (not listed). Ask about specific topics and relevant sources will be retrieved automatically.\n",
                total_count - MANIFEST_MAX_SOURCES
            )
        } else {
            String::new()
        };

        if estimated_full_tokens > MANIFEST_MAX_TOKENS {
            // Compact mode: titles + word count only, no summaries
            let mut manifest = header;
            for (i, source) in display_sources.iter().enumerate() {
                manifest.push_str(&format!(
                    "{}. {} ({} words)\n",
                    i + 1,
                    source.title,
                    source.word_count.unwrap_or(0),
                ));
            }
            manifest.push_str(&capped_footer);
            manifest.push_str(match scope_kind {
                ResolvedSourceScopeKind::All => {
                    "\nWhen asked about what sources you have, refer to the list above.\n"
                }
                ResolvedSourceScopeKind::Explicit => {
                    "\nWhen asked about the current chat scope, refer to the selected-source list above.\n"
                }
                ResolvedSourceScopeKind::None => "\n",
            });
            manifest
        } else {
            // Full mode: titles + summaries
            let mut manifest = header;
            for (i, source) in display_sources.iter().enumerate() {
                let summary_line = source
                    .summary
                    .as_deref()
                    .map(|s| {
                        if s.len() > 120 {
                            let mut end = 117;
                            while end > 0 && !s.is_char_boundary(end) {
                                end -= 1;
                            }
                            format!(" \u{2014} {}…", &s[..end])
                        } else {
                            format!(" \u{2014} {}", s)
                        }
                    })
                    .unwrap_or_default();

                manifest.push_str(&format!(
                    "{}. {} ({} words){}\n",
                    i + 1,
                    source.title,
                    source.word_count.unwrap_or(0),
                    summary_line,
                ));
            }
            manifest.push_str(&capped_footer);
            manifest.push_str(match scope_kind {
                ResolvedSourceScopeKind::All => {
                    "\nWhen asked about what sources you have or what's in this notebook, \
                     refer to the complete source list above.\n"
                }
                ResolvedSourceScopeKind::Explicit => {
                    "\nWhen asked about the current chat scope, refer to the selected-source list above.\n"
                }
                ResolvedSourceScopeKind::None => "\n",
            });
            manifest
        }
    }

    /// Format search results as context for the LLM.
    pub fn format_chunks(results: &[SearchResult]) -> String {
        if results.is_empty() {
            return "No relevant context found in the sources.".to_string();
        }

        let mut context = String::from("## Relevant Context\n\n");
        for (i, result) in results.iter().enumerate() {
            let source_id = &result.chunk.source_id;
            context.push_str(&format!(
                "[{}] (Source: {})\n{}\n\n",
                i + 1,
                source_id,
                result.chunk.content
            ));
        }
        context
    }

    /// Format conversation history for context.
    pub fn format_history(messages: &[Message], max_messages: usize) -> Vec<(String, String)> {
        let start = if messages.len() > max_messages {
            messages.len() - max_messages
        } else {
            0
        };
        messages[start..]
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, title: &str, summary: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: title.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: Some(42),
            metadata: None,
            summary: Some(summary.to_string()),
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn scoped_prompt_only_includes_selected_manifest_and_passages() {
        let selected = vec![source("s1", "Selected Source", "selected summary")];
        let prompt = ContextAssembler::build_system_prompt(
            None,
            "default",
            ResolvedSourceScopeKind::Explicit,
            &selected,
            &[ContextPassage {
                source_id: "s1".to_string(),
                title: "Selected Source".to_string(),
                content: "Selected evidence".to_string(),
            }],
        );

        assert!(prompt.contains("Selected Source"));
        assert!(prompt.contains("Selected evidence"));
        assert!(!prompt.contains("Unselected Source"));
        assert!(!prompt.contains("unselected summary"));
    }

    #[test]
    fn scoped_prompt_does_not_fallback_to_manifest_summaries() {
        let selected = vec![source("s1", "Selected Source", "selected summary")];
        let prompt = ContextAssembler::build_system_prompt(
            None,
            "default",
            ResolvedSourceScopeKind::Explicit,
            &selected,
            &[],
        );

        assert!(prompt.contains("Selected Source"));
        assert!(prompt.contains("Do not infer document details"));
        assert!(!prompt.contains("Use the source titles and summaries"));
    }

    #[test]
    fn none_scope_prompt_does_not_leak_manifest() {
        let prompt = ContextAssembler::build_system_prompt(
            None,
            "default",
            ResolvedSourceScopeKind::None,
            &[],
            &[],
        );

        assert!(prompt.contains("No notebook sources are selected"));
        assert!(!prompt.contains("This chat is scoped"));
        assert!(!prompt.contains("You have access to"));
    }
}
