use crate::types::{EpisodeMeta, EpisodeOutcome, VerificationStatus};
use stack_ids::TraceCtx;

/// Helper to convert `Option<&[&str]>` into owned data for `'static` closures,
/// and convert back to the reference form inside the closure.
pub(crate) fn to_owned_string_vec(opt: Option<&[&str]>) -> Option<Vec<String>> {
    opt.map(|items| items.iter().map(|item| item.to_string()).collect())
}

/// Convert `Option<Vec<String>>` back to `Option<Vec<&str>>` + `Option<&[&str]>`.
pub(crate) fn as_str_slice(opt: &Option<Vec<String>>) -> Option<Vec<&str>> {
    opt.as_ref()
        .map(|values| values.iter().map(|value| value.as_str()).collect())
}

pub(crate) fn merge_trace_ctx(
    metadata: Option<serde_json::Value>,
    trace_ctx: Option<&TraceCtx>,
) -> Option<serde_json::Value> {
    let Some(trace_ctx) = trace_ctx else {
        return metadata;
    };

    match metadata {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert(
                "trace_id".to_string(),
                serde_json::Value::String(trace_ctx.trace_id.clone()),
            );
            Some(serde_json::Value::Object(map))
        }
        Some(existing) => Some(serde_json::json!({
            "trace_id": trace_ctx.trace_id,
            "payload": existing,
        })),
        None => Some(serde_json::json!({
            "trace_id": trace_ctx.trace_id,
        })),
    }
}

fn describe_verification_status(status: &VerificationStatus) -> String {
    match status {
        VerificationStatus::Unverified => "unverified".to_string(),
        VerificationStatus::Verified { method, at } => {
            format!("verified via {method} at {at}")
        }
        VerificationStatus::Failed { reason, at } => {
            format!("verification failed at {at}: {reason}")
        }
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

pub(crate) fn build_episode_search_text(
    document_title: &str,
    document_context: &str,
    meta: &EpisodeMeta,
) -> String {
    let cause_text = if meta.cause_ids.is_empty() {
        "none".to_string()
    } else {
        meta.cause_ids.join(" ")
    };
    let experiment_text = meta.experiment_id.as_deref().unwrap_or("none");
    let verification_text = describe_verification_status(&meta.verification_status);
    let context_excerpt = truncate_chars(document_context, 2_000);

    format!(
        "document {document_title}\n\
         effect {effect}\n\
         outcome {outcome}\n\
         confidence {confidence:.3}\n\
         verification {verification}\n\
         experiment {experiment}\n\
         causes {causes}\n\
         context {context}",
        effect = meta.effect_type,
        outcome = meta.outcome.as_str(),
        confidence = meta.confidence,
        verification = verification_text,
        experiment = experiment_text,
        causes = cause_text,
        context = context_excerpt,
    )
}

pub(crate) fn verification_status_for_outcome(
    outcome: &EpisodeOutcome,
    experiment_id: Option<&str>,
) -> VerificationStatus {
    match outcome {
        EpisodeOutcome::Pending => VerificationStatus::Unverified,
        EpisodeOutcome::Confirmed | EpisodeOutcome::Refuted | EpisodeOutcome::Inconclusive => {
            VerificationStatus::Verified {
                method: experiment_id
                    .map(|id| format!("experiment:{id}"))
                    .unwrap_or_else(|| "manual_outcome_update".to_string()),
                at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            }
        }
    }
}
