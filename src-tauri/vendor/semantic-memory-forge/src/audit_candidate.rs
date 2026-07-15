//! Audit/explain-only candidate search boundary types.
//!
//! Forge owns verification truth. These types model candidate discovery through
//! semantic-memory/proveKV only for audit explanation; they cannot promote,
//! verify, or rank claims authoritatively.

use serde::{Deserialize, Serialize};

pub const FORGE_AUDIT_CANDIDATE_SEARCH_V1_SCHEMA: &str = "forge_audit_candidate_search_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeAuditCandidateSearchRequestV1 {
    pub query: String,
    pub scope: String,
    pub limit: usize,
    pub explain_only: bool,
}

impl ForgeAuditCandidateSearchRequestV1 {
    /// Validate that the request stays inside the explain-only Forge audit boundary.
    pub fn validate(&self) -> Result<(), String> {
        if !self.explain_only {
            return Err("Forge audit candidate search must be explain_only".into());
        }
        if self.query.trim().is_empty() {
            return Err("query must not be empty".into());
        }
        if self.scope.trim().is_empty() {
            return Err("scope must not be empty".into());
        }
        if self.limit == 0 {
            return Err("limit must be > 0".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeAuditCandidateSearchResultV1 {
    pub evidence_ref: String,
    pub summary: String,
    pub retrieval_backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_backend: Option<String>,
    pub candidate_only: bool,
    pub exact_rerank: bool,
    pub verified_by_forge: bool,
}

impl ForgeAuditCandidateSearchResultV1 {
    /// Validate that a retrieved item remains a candidate and does not claim Forge verification.
    pub fn validate_candidate_boundary(&self) -> Result<(), String> {
        if !self.candidate_only {
            return Err("audit search results must remain candidate_only".into());
        }
        if !self.exact_rerank {
            return Err("audit search results require semantic-memory exact rerank".into());
        }
        if self.verified_by_forge {
            return Err("candidate search result cannot claim Forge verification".into());
        }
        Ok(())
    }
}
