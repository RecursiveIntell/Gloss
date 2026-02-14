use crate::db::notebook_db::Message;
use crate::retrieval::hybrid_search::SearchResult;

/// Assemble the context for an LLM chat request.
pub struct ContextAssembler;

impl ContextAssembler {
    /// Build the system prompt with citation instructions and notebook context.
    pub fn build_system_prompt(custom_goal: Option<&str>, style: &str) -> String {
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

        // Citation instructions
        prompt.push_str(
            "When answering, cite sources using [1], [2], etc. based on the provided context. \
             Reference the specific source and section when citing. \
             Only cite information that is directly supported by the provided context.\n\n"
        );

        prompt
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
