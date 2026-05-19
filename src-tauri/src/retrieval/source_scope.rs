use crate::db::notebook_db::Source;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "ids", rename_all = "snake_case")]
pub enum SourceScope {
    All,
    Explicit(Vec<String>),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSourceScopeKind {
    All,
    Explicit,
    None,
}

#[derive(Debug, Clone)]
pub struct ResolvedSourceScope {
    kind: ResolvedSourceScopeKind,
    source_ids: Vec<String>,
    manifest_sources: Vec<Source>,
}

impl ResolvedSourceScope {
    pub fn kind(&self) -> ResolvedSourceScopeKind {
        self.kind
    }

    pub fn is_none(&self) -> bool {
        self.kind == ResolvedSourceScopeKind::None
    }

    pub fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    pub fn manifest_sources(&self) -> &[Source] {
        &self.manifest_sources
    }

    pub fn source_count(&self) -> usize {
        self.source_ids.len()
    }

    pub fn allows(&self, source_id: &str) -> bool {
        match self.kind {
            ResolvedSourceScopeKind::All => self.source_ids.iter().any(|id| id == source_id),
            ResolvedSourceScopeKind::Explicit => self.source_ids.iter().any(|id| id == source_id),
            ResolvedSourceScopeKind::None => false,
        }
    }
}

impl SourceScope {
    pub fn resolve(&self, all_sources: &[Source]) -> ResolvedSourceScope {
        match self {
            SourceScope::All => ResolvedSourceScope {
                kind: ResolvedSourceScopeKind::All,
                source_ids: all_sources.iter().map(|source| source.id.clone()).collect(),
                manifest_sources: all_sources.to_vec(),
            },
            SourceScope::Explicit(ids) => {
                let mut seen = HashSet::new();
                let manifest_sources: Vec<Source> = ids
                    .iter()
                    .filter(|id| seen.insert(id.as_str()))
                    .filter_map(|id| all_sources.iter().find(|source| source.id == *id).cloned())
                    .collect();

                if manifest_sources.is_empty() {
                    return ResolvedSourceScope {
                        kind: ResolvedSourceScopeKind::None,
                        source_ids: Vec::new(),
                        manifest_sources: Vec::new(),
                    };
                }

                ResolvedSourceScope {
                    kind: ResolvedSourceScopeKind::Explicit,
                    source_ids: manifest_sources
                        .iter()
                        .map(|source| source.id.clone())
                        .collect(),
                    manifest_sources,
                }
            }
            SourceScope::None => ResolvedSourceScope {
                kind: ResolvedSourceScopeKind::None,
                source_ids: Vec::new(),
                manifest_sources: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, title: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: title.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: Some(10),
            metadata: None,
            summary: Some(format!("summary for {title}")),
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn resolves_all_scope_without_dropping_sources() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved = SourceScope::All.resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::All);
        assert_eq!(resolved.source_ids(), &["a".to_string(), "b".to_string()]);
        assert_eq!(resolved.manifest_sources().len(), 2);
        assert!(resolved.allows("a"));
        assert!(resolved.allows("b"));
        assert!(!resolved.allows("deleted"));
    }

    #[test]
    fn resolves_explicit_scope_without_widening_to_all() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved = SourceScope::Explicit(vec!["b".to_string()]).resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::Explicit);
        assert_eq!(resolved.source_ids(), &["b".to_string()]);
        assert_eq!(resolved.manifest_sources()[0].title, "Beta");
        assert!(!resolved.allows("a"));
    }

    #[test]
    fn resolves_invalid_explicit_scope_to_none_instead_of_all() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved = SourceScope::Explicit(vec!["missing".to_string()]).resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::None);
        assert!(resolved.source_ids().is_empty());
        assert!(resolved.manifest_sources().is_empty());
        assert!(!resolved.allows("a"));
    }

    #[test]
    fn source_scope_none_returns_no_manifest_or_passages() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved = SourceScope::None.resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::None);
        assert!(resolved.source_ids().is_empty());
        assert!(resolved.manifest_sources().is_empty());
        assert!(!resolved.allows("a"));
        assert!(!resolved.allows("b"));
    }

    #[test]
    fn explicit_scope_dedupes_and_preserves_order_without_widening() {
        let all_sources = vec![
            source("a", "Alpha"),
            source("b", "Beta"),
            source("c", "Gamma"),
        ];

        let resolved =
            SourceScope::Explicit(vec!["b".to_string(), "b".to_string(), "a".to_string()])
                .resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::Explicit);
        assert_eq!(resolved.source_ids(), &["b".to_string(), "a".to_string()]);
        assert!(resolved.allows("a"));
        assert!(resolved.allows("b"));
        assert!(!resolved.allows("c"));
    }

    #[test]
    fn explicit_scope_with_one_valid_and_invalid_stays_bounded_to_valid() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved = SourceScope::Explicit(vec!["missing".to_string(), "a".to_string()])
            .resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::Explicit);
        assert_eq!(resolved.source_ids(), &["a".to_string()]);
        assert_eq!(resolved.manifest_sources().len(), 1);
        assert!(resolved.allows("a"));
        assert!(!resolved.allows("b"));
    }

    #[test]
    fn explicit_scope_treats_sql_fts_payload_as_data() {
        let all_sources = vec![source("a", "Alpha"), source("b", "Beta")];

        let resolved =
            SourceScope::Explicit(vec!["a') OR 1=1 --".to_string()]).resolve(&all_sources);

        assert_eq!(resolved.kind(), ResolvedSourceScopeKind::None);
        assert!(resolved.source_ids().is_empty());
        assert!(!resolved.allows("a"));
        assert!(!resolved.allows("b"));
    }
}
