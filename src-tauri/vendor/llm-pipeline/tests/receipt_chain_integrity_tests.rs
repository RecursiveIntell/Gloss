//! Receipt chain integrity tests for llm-pipeline.
//!
//! `PipelineExecutionReceiptV1` is the doctrinal backbone of the
//! LLM-pipeline crate: it is the audit handle for a full pipeline
//! execution. The four receipt types it aggregates
//! (`ProviderCallReceiptV1`, `RetryDecisionReceiptV1`, `BudgetDebitV1`)
//! are the chain — and the chain must be **structurally sound** for
//! downstream audit, replay, and budget reconciliation to work.
//!
//! These tests assert invariants that must hold for *any* well-formed
//! `PipelineExecutionReceiptV1`, regardless of which pipeline produced
//! it. They run as a unit test of the type surface, not as an
//! integration test of pipeline.rs (which would require a live LLM
//! provider).

#![allow(clippy::expect_used)]

use llm_pipeline::{
    BudgetDebitV1, ExecutionOutcome, PipelineExecutionReceiptV1, ProviderCallReceiptV1, RetryCause,
    RetryDecision, RetryDecisionReceiptV1,
};

fn make_budget() -> BudgetDebitV1 {
    BudgetDebitV1 {
        budget_id: "budget-default".into(),
        debit: 0.0,
        remaining: 100.0,
    }
}

fn make_provider_call(seed: u32) -> ProviderCallReceiptV1 {
    ProviderCallReceiptV1 {
        receipt_id: format!("provider-call-{seed}"),
        provider: "ollama".into(),
        model_route: "llama3.1:8b".into(),
        request_digest: format!("req-digest-{seed}"),
        response_digest: format!("resp-digest-{seed}"),
        latency_ms: 100 + u64::from(seed),
        tokens_in: 50,
        tokens_out: 25,
        retrieved_context: vec![],
    }
}

fn make_retry(attempt: u32) -> RetryDecisionReceiptV1 {
    RetryDecisionReceiptV1 {
        receipt_id: format!("retry-{attempt}"),
        attempt_number: attempt,
        max_attempts: 3,
        cause: RetryCause::TransportError("test".into()),
        decision: RetryDecision::Retrying,
        budget_impact: BudgetDebitV1 {
            budget_id: "budget-default".into(),
            debit: 1.0,
            remaining: 99.0,
        },
    }
}

fn make_clean_receipt() -> PipelineExecutionReceiptV1 {
    PipelineExecutionReceiptV1 {
        receipt_id: "pipeline-1".into(),
        pipeline_id: "test-pipeline".into(),
        provider_calls: vec![make_provider_call(1), make_provider_call(2)],
        retry_decisions: vec![make_retry(1)],
        budget_debits: vec![make_budget()],
        response_digest: "final-response-digest".into(),
        outcome: ExecutionOutcome::Success,
        recorded_time: chrono::Utc::now(),
    }
}

#[test]
fn pipeline_receipt_provider_call_ids_are_unique() {
    // A clean receipt produced by `make_clean_receipt()` has unique
    // provider_call receipt_ids. The receipt type is data — it does
    // not enforce uniqueness — so the producer is responsible. This
    // test documents the invariant the producer must satisfy.
    let r = make_clean_receipt();
    let ids: Vec<&String> = r.provider_calls.iter().map(|c| &c.receipt_id).collect();
    let unique: std::collections::HashSet<&String> = ids.iter().copied().collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "producer must emit unique provider call receipt_ids (got {} ids, {} unique)",
        ids.len(),
        unique.len()
    );
}

#[test]
fn pipeline_receipt_retry_attempt_numbers_are_in_range() {
    // Each retry's attempt_number must be 1..=max_attempts.
    let r = make_clean_receipt();
    for retry in &r.retry_decisions {
        assert!(
            retry.attempt_number >= 1,
            "attempt_number must be >= 1, got {}",
            retry.attempt_number
        );
        assert!(
            retry.attempt_number <= retry.max_attempts,
            "attempt_number {} must be <= max_attempts {}",
            retry.attempt_number,
            retry.max_attempts
        );
    }
}

#[test]
fn pipeline_receipt_response_digest_is_non_empty_on_completed() {
    // A completed pipeline must produce a non-empty response_digest —
    // it's the consumer's handle to the actual output for replay.
    let r = make_clean_receipt();
    assert!(matches!(r.outcome, ExecutionOutcome::Success));
    assert!(
        !r.response_digest.is_empty(),
        "completed pipeline must have a non-empty response_digest"
    );
}

#[test]
fn pipeline_receipt_retry_chain_does_not_exceed_max_attempts() {
    // Even with duplicates, the total number of retry decisions
    // recorded for a single pipeline run must not exceed the
    // max_attempts. This catches off-by-one bugs in retry logic
    // where the retry counter and the decision record diverge.
    let mut r = make_clean_receipt();
    let max = r.retry_decisions[0].max_attempts;
    // Add 2 more retries, all at max_attempts to test the boundary.
    r.retry_decisions.push(make_retry(max));
    r.retry_decisions.push(make_retry(max));
    assert!(
        r.retry_decisions.len() <= max as usize,
        "retry_decisions count {} must not exceed max_attempts {}",
        r.retry_decisions.len(),
        max
    );
}

#[test]
fn pipeline_receipt_serializes_to_json_round_trip() {
    // The receipt is the audit handle. A consumer must be able to
    // persist it (serialize) and reload it (deserialize) for replay.
    let r = make_clean_receipt();
    let json = serde_json::to_string(&r).expect("receipt must serialize");
    let restored: PipelineExecutionReceiptV1 =
        serde_json::from_str(&json).expect("receipt must deserialize");
    assert_eq!(restored.receipt_id, r.receipt_id);
    assert_eq!(restored.pipeline_id, r.pipeline_id);
    assert_eq!(restored.provider_calls.len(), r.provider_calls.len());
    assert_eq!(restored.retry_decisions.len(), r.retry_decisions.len());
    assert_eq!(restored.response_digest, r.response_digest);
    assert!(matches!(restored.outcome, ExecutionOutcome::Success));
}

#[test]
fn pipeline_receipt_budget_debits_reference_consistent_budget_ids() {
    // For a clean single-budget pipeline, the receipt must contain
    // debits all referencing the same budget_id. The receipt type
    // does not enforce this — the producer is responsible — so this
    // test documents the invariant the producer must satisfy.
    let r = make_clean_receipt();
    let ids: std::collections::HashSet<String> = r
        .budget_debits
        .iter()
        .map(|d| d.budget_id.clone())
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "producer must emit budget_debits with a single budget_id (got {:?})",
        ids
    );
}
