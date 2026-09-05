use crate::db::notebook_db::Message;
use crate::error::GlossError;

/// Rebuild request context before an explicitly edited user turn. The caller
/// retains every persisted message; only this request's history is bounded.
pub(super) fn history_before_rerun(
    mut history: Vec<Message>,
    conversation_id: &str,
    before_user_message_id: Option<&str>,
) -> Result<Vec<Message>, GlossError> {
    if let Some(target) = before_user_message_id {
        let position = history
            .iter()
            .position(|message| {
                message.id == target
                    && message.conversation_id == conversation_id
                    && message.role == "user"
            })
            .ok_or_else(|| {
                GlossError::Other(
                    "Rerun target is not a saved user message in this conversation".to_string(),
                )
            })?;
        history.truncate(position);
    }
    Ok(history)
}

/// Bind the optimistic frontend user row to its durable message identity.
pub(super) fn resolve_user_message_id(
    provided: Option<String>,
    assistant_message_id: &str,
) -> Result<String, GlossError> {
    let id = match provided {
        Some(id) => {
            let parsed = uuid::Uuid::parse_str(&id)
                .map_err(|_| GlossError::Other("User message ID must be a UUID".to_string()))?;
            if parsed.to_string() != id {
                return Err(GlossError::Other(
                    "User message ID must use canonical UUID form".to_string(),
                ));
            }
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };
    if id == assistant_message_id {
        return Err(GlossError::Other(
            "User and assistant message IDs must differ".to_string(),
        ));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, role: &str, content: &str) -> Message {
        Message {
            id: id.to_string(),
            conversation_id: "conversation".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            citations: None,
            model_used: None,
            tokens_prompt: None,
            tokens_response: None,
            created_at: String::new(),
        }
    }

    fn fixture() -> Vec<Message> {
        vec![
            message("greeting", "user", "HELLO_GLOSS"),
            message("answer", "assistant", "HELLO_GLOSS"),
            message("cancelled-question", "user", "Write 100 ocean facts"),
            message("later-question", "user", "A later question"),
        ]
    }

    #[test]
    fn rerun_excludes_superseded_and_later_turns_without_mutating_saved_history() {
        let saved = fixture();
        let context =
            history_before_rerun(saved.clone(), "conversation", Some("cancelled-question"))
                .unwrap();
        let ids: Vec<_> = context.iter().map(|message| message.id.as_str()).collect();
        assert_eq!(ids, vec!["greeting", "answer"]);
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[2].content, "Write 100 ocean facts");
    }

    #[test]
    fn ordinary_turn_keeps_full_history_and_first_turn_rerun_is_empty() {
        let context = history_before_rerun(fixture(), "conversation", None).unwrap();
        assert_eq!(context.len(), 4);
        let first = history_before_rerun(fixture(), "conversation", Some("greeting")).unwrap();
        assert!(first.is_empty());
    }

    #[test]
    fn unknown_assistant_and_foreign_conversation_targets_fail_closed() {
        for target in ["missing", "answer", ""] {
            assert!(history_before_rerun(fixture(), "conversation", Some(target)).is_err());
        }
        assert!(history_before_rerun(fixture(), "other", Some("greeting")).is_err());
    }

    #[test]
    fn persisted_targets_are_conversation_scoped_and_rejection_preserves_database() {
        let directory = tempfile::tempdir().unwrap();
        let db = crate::db::notebook_db::NotebookDb::open(&directory.path().join("notebook.db"))
            .unwrap();
        db.create_conversation("conversation").unwrap();
        db.create_conversation("other").unwrap();
        db.insert_message(&message("user", "user", "Saved question"))
            .unwrap();
        db.insert_message(&message("assistant", "assistant", "Saved answer"))
            .unwrap();
        let mut foreign = message("foreign-user", "user", "Other conversation");
        foreign.conversation_id = "other".to_string();
        db.insert_message(&foreign).unwrap();

        let before = serde_json::to_value(db.load_messages("conversation").unwrap()).unwrap();
        for target in ["missing", "assistant", "foreign-user"] {
            let result = history_before_rerun(
                db.load_messages("conversation").unwrap(),
                "conversation",
                Some(target),
            );
            assert!(result.is_err());
        }
        assert_eq!(
            serde_json::to_value(db.load_messages("conversation").unwrap()).unwrap(),
            before
        );
        assert_eq!(
            db.load_messages("other").unwrap()[0].content,
            foreign.content
        );
        let attempts: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM chat_attempts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(attempts, 0);
    }

    #[test]
    fn supplied_user_identity_is_exact_and_cannot_alias_assistant() {
        let id = "988f79eb-6fe1-4a2c-b815-828abc313bfb";
        assert_eq!(
            resolve_user_message_id(Some(id.to_string()), "assistant").unwrap(),
            id
        );
        assert!(resolve_user_message_id(Some(id.to_string()), id).is_err());
        assert!(resolve_user_message_id(Some("not-a-uuid".to_string()), "assistant").is_err());
        assert!(resolve_user_message_id(Some(id.to_uppercase()), "assistant").is_err());
        let generated = resolve_user_message_id(None, "assistant").unwrap();
        assert!(uuid::Uuid::parse_str(&generated).is_ok());
    }
}
