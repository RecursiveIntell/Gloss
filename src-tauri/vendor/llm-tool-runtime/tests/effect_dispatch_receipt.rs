#![allow(clippy::expect_used)]

use llm_tool_runtime::ToolEffectDispatchReceiptV1;
use stack_ids::{EffectCommitDecisionId, ToolEffectDispatchReceiptId};

#[test]
fn effect_dispatch_receipt_roundtrip() {
    let receipt = ToolEffectDispatchReceiptV1 {
        schema_version: "tool_effect_dispatch_receipt_v1".into(),
        tool_effect_dispatch_receipt_id: ToolEffectDispatchReceiptId::new(
            "tool-effect-dispatch-receipt-1",
        ),
        effect_commit_decision_id: EffectCommitDecisionId::new("effect-commit-decision-1"),
        tool_receipt_id: "tool-receipt-1".into(),
        provider_route: vec!["forge-pilot".into(), "llm-tool-runtime".into()],
        dispatch_state: "dispatched".into(),
        effect_execution_receipt_id: None,
        deadline_at_dispatch: "2026-03-15T12:05:00Z".into(),
        cancellation_reason: "".into(),
    };

    let json = serde_json::to_string(&receipt).expect("serialize");
    let _: ToolEffectDispatchReceiptV1 = serde_json::from_str(&json).expect("deserialize");
}
