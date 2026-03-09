//! Scope and partition primitives.
//!
//! `ScopeKey` is the canonical partition key for the stack. All scope-aware
//! operations (entity resolution, projection tracking, import routing) use
//! `ScopeKey` for partitioning.
//!
//! ## Namespace-to-ScopeKey migration
//!
//! Legacy code uses a bare `namespace: String` as the partition key. The
//! canonical migration rule is:
//!
//! ```text
//! legacy namespace "foo" → ScopeKey { namespace: "foo", domain: None, workspace_id: None, repo_id: None }
//! ```
//!
//! This mapping is deterministic and reversible via `ScopeKey::from_legacy_namespace()`
//! and `ScopeKey::to_legacy_namespace()`. All bridge, importer, and test code
//! must use these functions for namespace↔ScopeKey conversion.

use serde::{Deserialize, Serialize};

/// Multi-dimensional scope that bounds every runtime query and projection.
///
/// At minimum a `namespace` is required. Optional `domain`, `workspace_id`,
/// and `repo_id` narrow scope further.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    /// Primary namespace partition.
    pub namespace: String,
    /// Logical domain within the namespace (e.g. "code", "docs", "ops").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Workspace identifier for multi-tenant isolation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Repository identifier for code-scoped queries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
}

impl Scope {
    /// Create a scope with only a namespace.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            domain: None,
            workspace_id: None,
            repo_id: None,
        }
    }

    /// Builder: set the domain.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Builder: set the workspace id.
    pub fn with_workspace(mut self, id: impl Into<String>) -> Self {
        self.workspace_id = Some(id.into());
        self
    }

    /// Builder: set the repo id.
    pub fn with_repo(mut self, id: impl Into<String>) -> Self {
        self.repo_id = Some(id.into());
        self
    }

    /// Produce a `ScopeKey` for use in hash maps and equality checks.
    pub fn key(&self) -> ScopeKey {
        ScopeKey {
            namespace: self.namespace.clone(),
            domain: self.domain.clone(),
            workspace_id: self.workspace_id.clone(),
            repo_id: self.repo_id.clone(),
        }
    }
}

/// Compact, hashable representation of all scope dimensions.
///
/// This is the canonical partition key for the stack. Two `ScopeKey`s
/// are equal iff all four fields match exactly.
///
/// ## Display format
///
/// `namespace[/domain][@workspace_id][#repo_id]`
///
/// Examples:
/// - `prod` — namespace only
/// - `prod/code` — with domain
/// - `prod/code@ws1#myrepo` — fully specified
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScopeKey {
    pub namespace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
}

impl ScopeKey {
    /// Create from just a namespace (all other dimensions None).
    pub fn namespace_only(ns: impl Into<String>) -> Self {
        Self {
            namespace: ns.into(),
            domain: None,
            workspace_id: None,
            repo_id: None,
        }
    }

    /// Deterministic migration from a legacy bare namespace string.
    ///
    /// This is the canonical namespace→ScopeKey mapping. All bridge, importer,
    /// and test code must use this for legacy namespace conversion.
    ///
    /// Mapping: `"foo"` → `ScopeKey { namespace: "foo", domain: None, workspace_id: None, repo_id: None }`
    pub fn from_legacy_namespace(namespace: impl Into<String>) -> Self {
        Self::namespace_only(namespace)
    }

    /// Reverse mapping: extract the legacy namespace string.
    ///
    /// This is only valid for ScopeKeys that were created from a legacy namespace
    /// (i.e. all dimensions except namespace are None). For multi-dimensional
    /// scopes, use the `namespace` field directly.
    pub fn to_legacy_namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns true if this scope has only a namespace (no domain/workspace/repo).
    pub fn is_namespace_only(&self) -> bool {
        self.domain.is_none() && self.workspace_id.is_none() && self.repo_id.is_none()
    }
}

impl std::fmt::Display for ScopeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.namespace)?;
        if let Some(d) = &self.domain {
            write!(f, "/{d}")?;
        }
        if let Some(w) = &self.workspace_id {
            write!(f, "@{w}")?;
        }
        if let Some(r) = &self.repo_id {
            write!(f, "#{r}")?;
        }
        Ok(())
    }
}

/// Phase status for a feature or behavior.
///
/// Used in code comments and metadata to distinguish implemented features
/// from planned ones. Prevents confusion about what is actually working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    /// Fully implemented and tested.
    Current,
    /// Exists only for migration compatibility; will be removed.
    Compatibility,
    /// Planned for a future phase; not yet implemented.
    PhaseGated,
}

impl PhaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Compatibility => "compatibility",
            Self::PhaseGated => "phase_gated",
        }
    }
}

impl std::fmt::Display for PhaseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_key_equality() {
        let s1 = Scope::new("ns").with_repo("repo-a");
        let s2 = Scope::new("ns").with_repo("repo-a");
        assert_eq!(s1.key(), s2.key());
    }

    #[test]
    fn scope_key_inequality_different_repo() {
        let s1 = Scope::new("ns").with_repo("repo-a");
        let s2 = Scope::new("ns").with_repo("repo-b");
        assert_ne!(s1.key(), s2.key());
    }

    #[test]
    fn scope_key_display() {
        let s = Scope::new("prod")
            .with_domain("code")
            .with_workspace("ws1")
            .with_repo("myrepo");
        assert_eq!(s.key().to_string(), "prod/code@ws1#myrepo");
    }

    #[test]
    fn scope_key_display_namespace_only() {
        let sk = ScopeKey::namespace_only("default");
        assert_eq!(sk.to_string(), "default");
    }

    #[test]
    fn legacy_namespace_roundtrip() {
        let sk = ScopeKey::from_legacy_namespace("my-namespace");
        assert_eq!(sk.to_legacy_namespace(), "my-namespace");
        assert!(sk.is_namespace_only());
    }

    #[test]
    fn non_namespace_only_scope() {
        let sk = Scope::new("ns").with_domain("code").key();
        assert!(!sk.is_namespace_only());
    }

    #[test]
    fn scope_key_ordering() {
        let a = ScopeKey::namespace_only("aaa");
        let b = ScopeKey::namespace_only("bbb");
        assert!(a < b);
    }

    #[test]
    fn scope_key_serde_roundtrip() {
        let sk = Scope::new("ns")
            .with_domain("code")
            .with_workspace("ws")
            .key();
        let json = serde_json::to_string(&sk).unwrap();
        let back: ScopeKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sk);
    }

    #[test]
    fn scope_key_serde_skips_none() {
        let sk = ScopeKey::namespace_only("ns");
        let json = serde_json::to_string(&sk).unwrap();
        assert!(!json.contains("domain"));
        assert!(!json.contains("workspace_id"));
        assert!(!json.contains("repo_id"));
    }
}
