//! Versioned contracts for deterministic, source-grounded memory transitions.

use serde::{Deserialize, Serialize};

pub const MEMORY_TRANSITION_CANDIDATE_V1: &str = "memory_transition_candidate_v1";
pub const MEMORY_TRANSITION_VERIFICATION_V1: &str = "memory_transition_verification_v1";
pub const MEMORY_TRANSITION_RECORD_V1: &str = "memory_transition_record_v1";

/// Immutable raw evidence supplied to the transition compiler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceArtifactV1 {
    pub artifact_id: String,
    pub content: String,
    pub content_digest: String,
}

impl SourceArtifactV1 {
    pub fn new(artifact_id: impl Into<String>, content: impl Into<String>) -> Result<Self, String> {
        let artifact_id = artifact_id.into();
        let content = content.into();
        if artifact_id.trim().is_empty() {
            return Err("source artifact ID must not be empty".into());
        }
        let content_digest = blake3::hash(content.as_bytes()).to_hex().to_string();
        Ok(Self {
            artifact_id,
            content,
            content_digest,
        })
    }
}

/// Byte-exact reference into one immutable source artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SourceSpanRefV1 {
    pub artifact_id: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

impl SourceSpanRefV1 {
    pub fn new(
        artifact_id: impl Into<String>,
        start_byte: usize,
        end_byte: usize,
    ) -> Result<Self, String> {
        let artifact_id = artifact_id.into();
        if artifact_id.trim().is_empty() {
            return Err("source span artifact ID must not be empty".into());
        }
        if start_byte >= end_byte {
            return Err("source span range must be non-empty and increasing".into());
        }
        Ok(Self {
            artifact_id,
            start_byte,
            end_byte,
        })
    }
}

/// One proposed canonical assertion and its complete source grounding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionDraftV1 {
    pub assertion_id: String,
    pub namespace: String,
    pub content: String,
    pub source_spans: Vec<SourceSpanRefV1>,
    pub dependency_fact_ids: Vec<String>,
}

impl AssertionDraftV1 {
    pub fn new(
        assertion_id: impl Into<String>,
        namespace: impl Into<String>,
        content: impl Into<String>,
        source_spans: Vec<SourceSpanRefV1>,
        dependency_fact_ids: Vec<String>,
    ) -> Result<Self, String> {
        let value = Self {
            assertion_id: assertion_id.into(),
            namespace: namespace.into(),
            content: content.into(),
            source_spans,
            dependency_fact_ids,
        };
        if value.assertion_id.trim().is_empty() {
            return Err("assertion ID must not be empty".into());
        }
        if value.namespace.trim().is_empty() || value.content.is_empty() {
            return Err("assertion namespace and content must not be empty".into());
        }
        if value.source_spans.is_empty() {
            return Err("assertion must cite at least one exact source span".into());
        }
        if value
            .dependency_fact_ids
            .iter()
            .any(|id| id.trim().is_empty())
        {
            return Err("dependency fact IDs must not be empty".into());
        }
        Ok(value)
    }
}

/// Explicit replacement of one current authority head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersessionDraftV1 {
    pub target_fact_id: String,
    pub replacement_assertion_id: String,
}

impl SupersessionDraftV1 {
    pub fn new(
        target_fact_id: impl Into<String>,
        replacement_assertion_id: impl Into<String>,
    ) -> Result<Self, String> {
        let value = Self {
            target_fact_id: target_fact_id.into(),
            replacement_assertion_id: replacement_assertion_id.into(),
        };
        if value.target_fact_id.trim().is_empty()
            || value.replacement_assertion_id.trim().is_empty()
        {
            return Err("supersession target and replacement IDs must not be empty".into());
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransitionOperation {
    Append {
        assertion_id: String,
    },
    Supersede {
        draft: SupersessionDraftV1,
    },
    Retract {
        target_fact_id: String,
        reason: String,
        source_spans: Vec<SourceSpanRefV1>,
    },
}

/// Candidate proposed by extraction or an operator before canonical admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTransitionCandidateV1 {
    pub schema_version: String,
    pub candidate_id: String,
    pub source_artifacts: Vec<SourceArtifactV1>,
    pub required_source_spans: Vec<SourceSpanRefV1>,
    pub assertions: Vec<AssertionDraftV1>,
    pub operation: TransitionOperation,
    /// Current assertions in affected namespaces that the proposal explicitly retains.
    pub preserved_active_fact_ids: Vec<String>,
}

impl MemoryTransitionCandidateV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        candidate_id: impl Into<String>,
        source_artifacts: Vec<SourceArtifactV1>,
        required_source_spans: Vec<SourceSpanRefV1>,
        assertions: Vec<AssertionDraftV1>,
        operation: TransitionOperation,
        preserved_active_fact_ids: Vec<String>,
    ) -> Result<Self, String> {
        let value = Self {
            schema_version: MEMORY_TRANSITION_CANDIDATE_V1.into(),
            candidate_id: candidate_id.into(),
            source_artifacts,
            required_source_spans,
            assertions,
            operation,
            preserved_active_fact_ids,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MEMORY_TRANSITION_CANDIDATE_V1 {
            return Err(format!(
                "unsupported transition candidate schema '{}'",
                self.schema_version
            ));
        }
        if self.candidate_id.trim().is_empty() {
            return Err("transition candidate ID must not be empty".into());
        }
        if self.source_artifacts.is_empty() {
            return Err("transition candidate must retain at least one source artifact".into());
        }
        let artifacts: std::collections::BTreeMap<&str, &SourceArtifactV1> = self
            .source_artifacts
            .iter()
            .map(|artifact| (artifact.artifact_id.as_str(), artifact))
            .collect();
        if artifacts.len() != self.source_artifacts.len() {
            return Err("source artifact IDs must be unique".into());
        }
        for artifact in &self.source_artifacts {
            if artifact.artifact_id.trim().is_empty() {
                return Err("source artifact ID must not be empty".into());
            }
            let digest = blake3::hash(artifact.content.as_bytes())
                .to_hex()
                .to_string();
            if digest != artifact.content_digest {
                return Err(format!(
                    "source artifact '{}' content digest mismatch",
                    artifact.artifact_id
                ));
            }
        }
        if self.required_source_spans.is_empty() {
            return Err("transition candidate must declare required source spans".into());
        }
        if self
            .preserved_active_fact_ids
            .iter()
            .any(|id| id.trim().is_empty())
        {
            return Err("preserved active fact IDs must not be empty".into());
        }
        let mut all_spans: Vec<&SourceSpanRefV1> = self.required_source_spans.iter().collect();
        all_spans.extend(
            self.assertions
                .iter()
                .flat_map(|assertion| assertion.source_spans.iter()),
        );
        if let TransitionOperation::Retract { source_spans, .. } = &self.operation {
            all_spans.extend(source_spans);
        }
        for span in all_spans {
            let artifact = artifacts.get(span.artifact_id.as_str()).ok_or_else(|| {
                format!(
                    "source span references missing source artifact '{}'",
                    span.artifact_id
                )
            })?;
            if span.start_byte >= span.end_byte
                || span.end_byte > artifact.content.len()
                || !artifact.content.is_char_boundary(span.start_byte)
                || !artifact.content.is_char_boundary(span.end_byte)
            {
                return Err(format!(
                    "source span {}:{}..{} is outside UTF-8 boundaries",
                    span.artifact_id, span.start_byte, span.end_byte
                ));
            }
        }
        let assertion_ids: std::collections::BTreeSet<&str> = self
            .assertions
            .iter()
            .map(|assertion| assertion.assertion_id.as_str())
            .collect();
        if assertion_ids.len() != self.assertions.len() {
            return Err("assertion IDs must be unique".into());
        }
        let assertion_id = match &self.operation {
            TransitionOperation::Append { assertion_id } => Some(assertion_id.as_str()),
            TransitionOperation::Supersede { draft } => {
                Some(draft.replacement_assertion_id.as_str())
            }
            TransitionOperation::Retract {
                target_fact_id,
                reason,
                source_spans,
            } => {
                if target_fact_id.trim().is_empty()
                    || reason.trim().is_empty()
                    || source_spans.is_empty()
                {
                    return Err(
                        "retraction requires a target, reason, and exact source spans".into(),
                    );
                }
                None
            }
        };
        if let Some(assertion_id) = assertion_id {
            let matching = self
                .assertions
                .iter()
                .filter(|draft| draft.assertion_id == assertion_id)
                .count();
            if matching != 1 {
                return Err("operation must reference exactly one assertion draft".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDisposition {
    Commit,
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationScore {
    pub satisfied: u64,
    pub total: u64,
}

impl VerificationScore {
    pub fn is_complete(self) -> bool {
        self.satisfied == self.total
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmittedSourceSpanV1 {
    pub span: SourceSpanRefV1,
    pub exact_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedAssertionSpanV1 {
    pub assertion_id: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub exact_text: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveHeadSimulationV1 {
    pub consistent: bool,
    pub replaced_fact_id: Option<String>,
    pub projected_active_fact_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySimulationV1 {
    pub requested_fact_ids: Vec<String>,
    pub missing_or_inactive_fact_ids: Vec<String>,
    pub preserved_edge_ids: Vec<String>,
}

/// Deterministic compiler result. It intentionally carries no wall-clock fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTransitionVerificationV1 {
    pub schema_version: String,
    pub candidate_id: String,
    pub candidate_digest: String,
    pub source_digest: String,
    pub coverage: VerificationScore,
    pub preservation: VerificationScore,
    pub faithfulness: VerificationScore,
    pub omitted_spans: Vec<OmittedSourceSpanV1>,
    pub unsupported_spans: Vec<UnsupportedAssertionSpanV1>,
    pub omitted_active_fact_ids: Vec<String>,
    pub diagnostics: Vec<String>,
    pub active_head_simulation: ActiveHeadSimulationV1,
    pub dependency_simulation: DependencySimulationV1,
    pub disposition: TransitionDisposition,
    pub verification_digest: String,
}

/// Immutable audit/quarantine row adjacent to the existing authority journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTransitionRecordV1 {
    pub schema_version: String,
    pub record_id: String,
    pub caller_idempotency_key: String,
    pub principal: String,
    pub caller_id: String,
    pub candidate_digest: String,
    pub candidate: MemoryTransitionCandidateV1,
    pub verification: MemoryTransitionVerificationV1,
    pub disposition: TransitionDisposition,
    pub authority_receipt_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // wire shape is already published; boxing would break it
pub enum MemoryTransitionOutcomeV1 {
    Committed {
        record: MemoryTransitionRecordV1,
        verification: MemoryTransitionVerificationV1,
        authority_receipt: crate::AuthorityReceiptV1,
    },
    Quarantined {
        record: MemoryTransitionRecordV1,
    },
}
