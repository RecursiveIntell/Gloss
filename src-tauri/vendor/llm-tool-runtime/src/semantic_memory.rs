//! Semantic-memory-facing contracts for searchable tool observations.

use crate::{ToolError, ToolErrorClass};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Serializable record written to semantic-memory for a completed or failed tool run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolObservationMemoryRecordV1 {
    /// Tool descriptor name.
    pub tool_name: String,
    /// Stable invocation identifier for this tool call.
    pub invocation_id: String,
    /// Optional session identifier used to scope retrieval.
    pub session_id: Option<String>,
    /// Retrieval scope/namespace; authorization decisions must not be based on similarity.
    pub scope: String,
    /// Searchable human-readable summary of the observation.
    pub summary: String,
    /// Tool status, such as `success`, `error`, or `cancelled`.
    pub status: String,
    /// Optional RFC3339 start timestamp.
    pub started_at: Option<String>,
    /// Optional RFC3339 completion timestamp.
    pub completed_at: Option<String>,
    /// Optional durable tool receipt id.
    pub receipt_id: Option<String>,
}

/// Semantic-memory derived-candidate receipt summary surfaced by searches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolObservationDerivedCandidateTraceV1 {
    /// Candidate backend name reported by semantic-memory.
    pub candidate_backend: String,
    /// Codec family, for example `provekv_pool`.
    pub codec_family: Option<String>,
    /// Derived generation id if a generation was used.
    pub generation_id: Option<String>,
    /// Whether exact f32 rerank was performed or required.
    pub exact_rerank: bool,
    /// Whether approximate candidates influenced the pre-rerank set.
    pub approximate: bool,
    /// Receipted fallback reason, if any.
    pub fallback: Option<String>,
}

/// Result of a similar-observation query, including candidate provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolObservationMemorySearchV1 {
    /// Matching observations, already scoped by the memory implementation.
    pub observations: Vec<ToolObservationMemoryRecordV1>,
    /// Candidate backend receipts; candidate-only is never authorization evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_candidate_receipts: Vec<ToolObservationDerivedCandidateTraceV1>,
}

/// Minimal trait for semantic-memory-backed tool observation storage/retrieval.
#[async_trait]
pub trait ToolObservationMemory: Send + Sync {
    /// Store a tool observation through the configured memory substrate.
    async fn store_tool_observation(
        &self,
        record: ToolObservationMemoryRecordV1,
    ) -> Result<(), ToolError>;

    /// Retrieve similar prior observations through semantic-memory; candidate-only remains non-authoritative.
    async fn find_similar_tool_observations(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
    ) -> Result<ToolObservationMemorySearchV1, ToolError>;
}

/// Small test/demonstration implementation that mimics scoped semantic-memory retrieval.
#[derive(Debug, Default)]
pub struct InMemoryToolObservationMemory {
    records: Mutex<Vec<ToolObservationMemoryRecordV1>>,
}

impl InMemoryToolObservationMemory {
    /// Returns all records currently stored in the in-memory implementation.
    pub fn records(&self) -> Vec<ToolObservationMemoryRecordV1> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl ToolObservationMemory for InMemoryToolObservationMemory {
    async fn store_tool_observation(
        &self,
        record: ToolObservationMemoryRecordV1,
    ) -> Result<(), ToolError> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(record);
        Ok(())
    }

    async fn find_similar_tool_observations(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
    ) -> Result<ToolObservationMemorySearchV1, ToolError> {
        if limit == 0 {
            return Err(ToolError::new(
                ToolErrorClass::ProviderContract,
                "limit must be greater than zero for tool observation search",
            ));
        }
        let query_lc = query.to_ascii_lowercase();
        let observations = self
            .records()
            .into_iter()
            .filter(|record| record.scope == scope)
            .filter(|record| {
                query_lc.is_empty() || record.summary.to_ascii_lowercase().contains(&query_lc)
            })
            .take(limit)
            .collect();
        Ok(ToolObservationMemorySearchV1 {
            observations,
            derived_candidate_receipts: Vec::new(),
        })
    }
}
