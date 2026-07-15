//! Query-aware compressed working-set selection.

#[cfg(feature = "no_std")]
use alloc::{string::String, vec::Vec};
#[cfg(not(feature = "no_std"))]
use std::{string::String, vec::Vec};

use crate::error::ScorerResult;
use crate::trait_def::{CompressedScorer, ProgressiveCompressedScorer, ProgressiveScoredCandidate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRole {
    SharedCold,
    AgentHot,
    RecentGuard,
    SinkGuard,
    RetrievalHeadLongRange,
    ExactFallback,
}

#[derive(Debug, Clone)]
pub struct CompressedPage<P> {
    pub page_id: String,
    pub layer: u32,
    pub head: u32,
    pub token_start: u32,
    pub token_end: u32,
    pub role: PageRole,
    pub codec_profile_digest: String,
    pub payload: P,
    pub exact_shadow_digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GuardPolicy {
    pub always_include_recent: usize,
    pub always_include_sink: bool,
}

#[derive(Debug, Clone)]
pub struct FallbackPolicy {
    pub require_exact_when_uncertain: bool,
    pub uncertainty_epsilon: f32,
}

#[derive(Debug, Clone)]
pub struct CacheRuntimePolicy {
    pub top_k: usize,
    pub oversample: usize,
    pub guard: GuardPolicy,
    pub fallback: FallbackPolicy,
}

impl Default for CacheRuntimePolicy {
    fn default() -> Self {
        Self {
            top_k: 8,
            oversample: 4,
            guard: GuardPolicy {
                always_include_recent: 0,
                always_include_sink: true,
            },
            fallback: FallbackPolicy {
                require_exact_when_uncertain: true,
                uncertainty_epsilon: 0.0,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkingSetSelectionReceipt {
    pub schema_version: String,
    pub candidate_count: usize,
    pub coarse_count: usize,
    pub margin_band_count: usize,
    pub refined_count: usize,
    pub guard_count: usize,
    pub exact_fallback_required: bool,
    pub decoded_values: usize,
}

#[derive(Debug, Clone)]
pub struct WorkingSetSelection {
    pub candidates: Vec<ProgressiveScoredCandidate>,
    pub receipt: WorkingSetSelectionReceipt,
}

pub struct CompressedWorkingSet<S: CompressedScorer> {
    pub pages: Vec<CompressedPage<S::Compressed>>,
    pub scorer: S,
    pub policy: CacheRuntimePolicy,
}

impl<S> CompressedWorkingSet<S>
where
    S: CompressedScorer,
    S::Compressed: Clone,
{
    pub fn new(scorer: S, policy: CacheRuntimePolicy) -> Self {
        Self {
            pages: Vec::new(),
            scorer,
            policy,
        }
    }

    pub fn push_page(&mut self, page: CompressedPage<S::Compressed>) {
        self.pages.push(page);
    }

    pub fn select(&self, query: &[f32]) -> ScorerResult<WorkingSetSelection> {
        let prepared = self.scorer.prepare_query(query)?;
        let mut scored: Vec<ProgressiveScoredCandidate> = Vec::with_capacity(self.pages.len());
        let mut guard_count = 0usize;
        for (idx, page) in self.pages.iter().enumerate() {
            let mut score = self.scorer.score_coarse(&prepared, &page.payload)?;
            if matches!(page.role, PageRole::RecentGuard | PageRole::SinkGuard) {
                score.score = f32::INFINITY;
                guard_count += 1;
            }
            scored.push(ProgressiveScoredCandidate::new(idx, score));
        }
        scored.sort_by(|a, b| {
            b.score
                .score
                .partial_cmp(&a.score.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        let keep = self
            .policy
            .top_k
            .saturating_mul(self.policy.oversample.max(1))
            .min(scored.len());
        let kth_score = scored
            .get(self.policy.top_k.saturating_sub(1))
            .map(|c| c.score.score)
            .unwrap_or(f32::NEG_INFINITY);
        let eps = self.policy.fallback.uncertainty_epsilon;
        let margin_band_count = scored
            .iter()
            .filter(|c| {
                c.score.lower_bound() <= kth_score + eps && c.score.upper_bound() >= kth_score - eps
            })
            .count();
        scored.truncate(keep.max(self.policy.top_k).min(scored.len()));
        let payloads: Vec<S::Compressed> = self.pages.iter().map(|p| p.payload.clone()).collect();
        let refined_count = scored.len();
        self.scorer
            .refine_candidates(&prepared, &mut scored, &payloads)?;
        for candidate in scored.iter_mut() {
            if let Some(page) = self.pages.get(candidate.idx) {
                if matches!(page.role, PageRole::RecentGuard | PageRole::SinkGuard) {
                    candidate.score.score = f32::INFINITY;
                }
            }
        }
        scored.sort_by(|a, b| {
            b.score
                .score
                .partial_cmp(&a.score.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        let exact_fallback_required = self.policy.fallback.require_exact_when_uncertain
            && margin_band_count > self.policy.top_k;
        scored.truncate(self.policy.top_k.min(scored.len()));
        Ok(WorkingSetSelection {
            receipt: WorkingSetSelectionReceipt {
                schema_version: "compressed_working_set_selection_v1".into(),
                candidate_count: self.pages.len(),
                coarse_count: self.pages.len(),
                margin_band_count,
                refined_count,
                guard_count,
                exact_fallback_required,
                decoded_values: scored.len(),
            },
            candidates: scored,
        })
    }
}
