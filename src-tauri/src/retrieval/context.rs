use crate::db::notebook_db::{Message, Source};
use crate::retrieval::hybrid_search::SearchResult;

/// Maximum estimated tokens for the source manifest before switching to compact mode.
const MANIFEST_MAX_TOKENS: usize = 2000;
/// Maximum number of sources to include in the manifest.
/// Beyond this, the manifest notes how many were omitted.
const MANIFEST_MAX_SOURCES: usize = 50;

/// Assemble the context for an LLM chat request.
pub struct ContextAssembler;

impl ContextAssembler {
    /// Build the system prompt. Source context is appended when present so it
    /// lives in the system message — not in user messages — which keeps history
    /// clean across turns.
    ///
    /// `all_sources` is the full list of sources for the manifest.
    /// `source_context` is the content of SELECTED sources for grounding.
    pub fn build_system_prompt(
        custom_goal: Option<&str>,
        style: &str,
        all_sources: &[Source],
        source_context: &[(String, String)],
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

        // Source manifest — tells the LLM what's in the notebook
        if !all_sources.is_empty() {
            prompt.push_str(&Self::build_source_manifest(all_sources));
            prompt.push('\n');
        }

        // Selected source content + citation instructions
        if !source_context.is_empty() {
            prompt.push_str("Retrieved passages:\n\n");
            for (i, (title, content)) in source_context.iter().enumerate() {
                prompt.push_str(&format!("[{}] {}\n{}\n\n", i + 1, title, content));
            }
            prompt.push_str(
                "When answering, cite the sources above using [1], [2], etc. \
                 Only cite information directly supported by these sources. \
                 Be concise.\n",
            );
        } else if !all_sources.is_empty() {
            // Sources exist but no passages were retrieved for this query.
            // Still direct the model to use its knowledge of the source list.
            prompt.push_str(
                "No specific passages were retrieved for this query, but you have access \
                 to the sources listed above. Use the source titles and summaries to provide \
                 a helpful response. If you cannot answer from the source information available, \
                 say so honestly and suggest the user refine their question.\n",
            );
        }

        prompt
    }

    /// Build a compact source manifest listing ALL sources in the notebook.
    /// Uses compact mode (titles only) when the full manifest would exceed
    /// the token budget.
    fn build_source_manifest(sources: &[Source]) -> String {
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
            "You have access to {} sources in this notebook{}:\n",
            total_count,
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
            manifest.push_str(
                "\nWhen asked about what sources you have, refer to the list above.\n",
            );
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
                            format!(" \u{2014} {}…", &s[..117])
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
            manifest.push_str(
                "\nWhen asked about what sources you have or what's in this notebook, \
                 refer to the complete source list above.\n",
            );
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
