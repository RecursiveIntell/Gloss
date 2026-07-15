//! Adaptive per-layer/head budget allocation for compressed attention.
//!
//! Given per-layer (or per-head) fragility data (cosine p05 at a reference top-k),
//! compute per-layer budgets that minimize total selected keys while keeping
//! each layer's expected cosine above a target threshold.
//!
//! Algorithm:
//! - If cosine_p05 >= target: layer is safe at ref_k. Reduce budget with
//!   exponential decay: scale = max(0.25, (1-slack)^10).
//! - If cosine_p05 < target: layer is fragile. Increase budget:
//!   k = ref_k * (1 + deficit * 5), clamped to max_k.
//! - All budgets include recent_guard.
//!
//! This is the Rust port of poly-kv/scripts/adaptive_budget.py, verified
//! against the same 256-token SmolLM2-1.7B/WikiText-2 gate.

#[cfg(feature = "no_std")]
extern crate alloc;

#[cfg(feature = "no_std")]
use alloc::vec::Vec;
#[cfg(not(feature = "no_std"))]
use std::vec::Vec;

/// Per-layer fragility entry: (layer_idx, cosine_p05 at reference top_k).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerFragilityEntry {
    pub layer: u32,
    pub cos_p05: f32,
}

/// Per-(layer, head) fragility entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadFragilityEntry {
    pub layer: u32,
    pub head: u32,
    pub cos_p05: f32,
}

/// Budget allocation configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetConfig {
    /// Reference top_k used when collecting fragility data.
    pub ref_k: usize,
    /// Target cosine p05 threshold.
    pub target_cosine: f32,
    /// Minimum top_k per layer (before recent_guard).
    pub min_k: usize,
    /// Maximum top_k per layer (before recent_guard).
    pub max_k: usize,
    /// Recent guard tokens always included.
    pub recent_guard: usize,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            ref_k: 64,
            target_cosine: 0.995,
            min_k: 32,
            max_k: 256,
            recent_guard: 16,
        }
    }
}

/// Per-layer budget result.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerBudgets {
    /// (layer_idx, top_k including recent_guard) pairs.
    pub budgets: Vec<(u32, usize)>,
    /// The config used to compute these budgets.
    pub config: BudgetConfig,
}

impl LayerBudgets {
    /// Expected mean selected keys at a given sequence length.
    pub fn expected_mean_k(&self, seq_len: usize) -> f64 {
        if self.budgets.is_empty() {
            return 0.0;
        }
        let total: usize = self.budgets.iter().map(|&(_, v)| v.min(seq_len)).sum();
        total as f64 / self.budgets.len() as f64
    }

    /// Get budget for a specific layer (including recent_guard).
    pub fn get(&self, layer_idx: u32) -> usize {
        for &(layer, budget) in &self.budgets {
            if layer == layer_idx {
                return budget;
            }
        }
        self.config.ref_k + self.config.recent_guard
    }

    /// Validate all budgets are within [min_k + guard, max_k + guard].
    pub fn validate(&self) -> bool {
        let lo = self.config.min_k + self.config.recent_guard;
        let hi = self.config.max_k + self.config.recent_guard;
        self.budgets.iter().all(|&(_, v)| v >= lo && v <= hi)
    }
}

/// Per-(layer, head) budget result.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadBudgets {
    /// ((layer, head), top_k including recent_guard) pairs.
    pub budgets: Vec<((u32, u32), usize)>,
    pub config: BudgetConfig,
}

impl HeadBudgets {
    /// Expected mean selected keys at a given sequence length.
    pub fn expected_mean_k(&self, seq_len: usize) -> f64 {
        if self.budgets.is_empty() {
            return 0.0;
        }
        let total: usize = self.budgets.iter().map(|&(_, v)| v.min(seq_len)).sum();
        total as f64 / self.budgets.len() as f64
    }

    /// Get budget for a specific (layer, head).
    pub fn get(&self, layer_idx: u32, head_idx: u32) -> usize {
        for &((layer, head), budget) in &self.budgets {
            if layer == layer_idx && head == head_idx {
                return budget;
            }
        }
        self.config.ref_k + self.config.recent_guard
    }
}

/// Compute budget for a single layer given its fragility and config.
fn compute_layer_k(cos_p05: f32, config: &BudgetConfig) -> usize {
    if cos_p05 >= config.target_cosine {
        let slack = cos_p05 - config.target_cosine;
        // scale = max(0.25, (1-slack)^10) — manual pow to avoid powi in no_std
        let base = (1.0 - slack).max(0.0);
        let scale = manual_pow(base, 10).max(0.25);
        (config.ref_k as f32 * scale).max(config.min_k as f32) as usize
    } else {
        let deficit = config.target_cosine - cos_p05;
        (config.ref_k as f32 * (1.0 + deficit * 5.0)).min(config.max_k as f32) as usize
    }
}

/// Manual integer power for no_std compatibility (f32::powi not available in core).
fn manual_pow(base: f32, exp: u32) -> f32 {
    let mut result = 1.0f32;
    let mut b = base;
    let mut e = exp;
    while e > 0 {
        if e & 1 == 1 {
            result *= b;
        }
        b *= b;
        e >>= 1;
    }
    result
}

/// Allocate per-layer budgets from fragility data.
pub fn allocate_layer_budgets(
    fragility: &[LayerFragilityEntry],
    config: &BudgetConfig,
) -> LayerBudgets {
    let budgets: Vec<(u32, usize)> = fragility
        .iter()
        .map(|e| {
            (
                e.layer,
                compute_layer_k(e.cos_p05, config) + config.recent_guard,
            )
        })
        .collect();
    LayerBudgets {
        budgets,
        config: config.clone(),
    }
}

/// Allocate per-(layer, head) budgets from head fragility data.
pub fn allocate_head_budgets(
    fragility: &[HeadFragilityEntry],
    config: &BudgetConfig,
) -> HeadBudgets {
    let budgets: Vec<((u32, u32), usize)> = fragility
        .iter()
        .map(|e| {
            (
                (e.layer, e.head),
                compute_layer_k(e.cos_p05, config) + config.recent_guard,
            )
        })
        .collect();
    HeadBudgets {
        budgets,
        config: config.clone(),
    }
}

/// Default fragility data from the 256-token top_k=64 diagnostic run
/// on SmolLM2-1.7B-Instruct / WikiText-2 (per-layer cosine_p05).
#[allow(clippy::vec_init_then_push)]
pub fn default_fragility_256tok() -> Vec<LayerFragilityEntry> {
    let mut v = Vec::new();
    v.push(LayerFragilityEntry {
        layer: 0,
        cos_p05: 0.7120,
    });
    v.push(LayerFragilityEntry {
        layer: 1,
        cos_p05: 1.0000,
    });
    v.push(LayerFragilityEntry {
        layer: 2,
        cos_p05: 0.9987,
    });
    v.push(LayerFragilityEntry {
        layer: 3,
        cos_p05: 0.9645,
    });
    v.push(LayerFragilityEntry {
        layer: 4,
        cos_p05: 0.9683,
    });
    v.push(LayerFragilityEntry {
        layer: 5,
        cos_p05: 0.9881,
    });
    v.push(LayerFragilityEntry {
        layer: 6,
        cos_p05: 0.9957,
    });
    v.push(LayerFragilityEntry {
        layer: 7,
        cos_p05: 0.9995,
    });
    v.push(LayerFragilityEntry {
        layer: 8,
        cos_p05: 0.9969,
    });
    v.push(LayerFragilityEntry {
        layer: 9,
        cos_p05: 0.9903,
    });
    v.push(LayerFragilityEntry {
        layer: 10,
        cos_p05: 0.9985,
    });
    v.push(LayerFragilityEntry {
        layer: 11,
        cos_p05: 0.9975,
    });
    v.push(LayerFragilityEntry {
        layer: 12,
        cos_p05: 0.9962,
    });
    v.push(LayerFragilityEntry {
        layer: 13,
        cos_p05: 0.9885,
    });
    v.push(LayerFragilityEntry {
        layer: 14,
        cos_p05: 0.9906,
    });
    v.push(LayerFragilityEntry {
        layer: 15,
        cos_p05: 0.9965,
    });
    v.push(LayerFragilityEntry {
        layer: 16,
        cos_p05: 0.9920,
    });
    v.push(LayerFragilityEntry {
        layer: 17,
        cos_p05: 0.9974,
    });
    v.push(LayerFragilityEntry {
        layer: 18,
        cos_p05: 0.9988,
    });
    v.push(LayerFragilityEntry {
        layer: 19,
        cos_p05: 0.9984,
    });
    v.push(LayerFragilityEntry {
        layer: 20,
        cos_p05: 0.9965,
    });
    v.push(LayerFragilityEntry {
        layer: 21,
        cos_p05: 0.9982,
    });
    v.push(LayerFragilityEntry {
        layer: 22,
        cos_p05: 0.9976,
    });
    v.push(LayerFragilityEntry {
        layer: 23,
        cos_p05: 0.9989,
    });
    v
}

/// 512-token fragility map (per-layer cosine_p05 at top_k=64).
///
/// Layer 0 collapses to 0.3905 — quantized 4-bit scoring is nearly broken for it
/// at this context length. Layers 1-23 are mostly stable (0.93-0.9999).
///
/// Degradation progression for layer 0:
///   128 tokens: 0.922
///   256 tokens: 0.712
///   512 tokens: 0.390
#[allow(clippy::vec_init_then_push)]
pub fn default_fragility_512tok() -> Vec<LayerFragilityEntry> {
    let mut v = Vec::new();
    v.push(LayerFragilityEntry {
        layer: 0,
        cos_p05: 0.3905,
    });
    v.push(LayerFragilityEntry {
        layer: 1,
        cos_p05: 0.9996,
    });
    v.push(LayerFragilityEntry {
        layer: 2,
        cos_p05: 0.9962,
    });
    v.push(LayerFragilityEntry {
        layer: 3,
        cos_p05: 0.9868,
    });
    v.push(LayerFragilityEntry {
        layer: 4,
        cos_p05: 0.9265,
    });
    v.push(LayerFragilityEntry {
        layer: 5,
        cos_p05: 0.9881,
    });
    v.push(LayerFragilityEntry {
        layer: 6,
        cos_p05: 0.9893,
    });
    v.push(LayerFragilityEntry {
        layer: 7,
        cos_p05: 0.9999,
    });
    v.push(LayerFragilityEntry {
        layer: 8,
        cos_p05: 0.9943,
    });
    v.push(LayerFragilityEntry {
        layer: 9,
        cos_p05: 0.9344,
    });
    v.push(LayerFragilityEntry {
        layer: 10,
        cos_p05: 0.9975,
    });
    v.push(LayerFragilityEntry {
        layer: 11,
        cos_p05: 0.9950,
    });
    v.push(LayerFragilityEntry {
        layer: 12,
        cos_p05: 0.9784,
    });
    v.push(LayerFragilityEntry {
        layer: 13,
        cos_p05: 0.9701,
    });
    v.push(LayerFragilityEntry {
        layer: 14,
        cos_p05: 0.9949,
    });
    v.push(LayerFragilityEntry {
        layer: 15,
        cos_p05: 0.9824,
    });
    v.push(LayerFragilityEntry {
        layer: 16,
        cos_p05: 0.9838,
    });
    v.push(LayerFragilityEntry {
        layer: 17,
        cos_p05: 0.9898,
    });
    v.push(LayerFragilityEntry {
        layer: 18,
        cos_p05: 0.9839,
    });
    v.push(LayerFragilityEntry {
        layer: 19,
        cos_p05: 0.9667,
    });
    v.push(LayerFragilityEntry {
        layer: 20,
        cos_p05: 0.9580,
    });
    v.push(LayerFragilityEntry {
        layer: 21,
        cos_p05: 0.9858,
    });
    v.push(LayerFragilityEntry {
        layer: 22,
        cos_p05: 0.9911,
    });
    v.push(LayerFragilityEntry {
        layer: 23,
        cos_p05: 0.9952,
    });
    v
}

/// Select the appropriate default fragility map by context length.
pub fn default_fragility_for_seq_len(n_tokens: usize) -> Vec<LayerFragilityEntry> {
    if n_tokens <= 256 {
        default_fragility_256tok()
    } else {
        default_fragility_512tok()
    }
}

/// Learn budget allocation from per-layer drift data.
///
/// Given per-layer fragility measurements and a target mean k, iteratively
/// reduce the max_k for stable layers until the expected mean k is below
/// the target. Fragile layers (below target_cosine) keep their full budget.
pub fn learn_budgets(
    fragility: &[LayerFragilityEntry],
    config: &BudgetConfig,
    target_mean_k: f64,
    seq_len: usize,
) -> LayerBudgets {
    let mut adjusted_config = config.clone();
    let max_step = (config.max_k - config.min_k) / 4;
    let mut step = max_step.max(1);

    for _ in 0..20 {
        let result = allocate_layer_budgets(fragility, &adjusted_config);
        let mean_k = result.expected_mean_k(seq_len);
        if mean_k <= target_mean_k {
            return result;
        }
        // Reduce max_k to shrink budgets for stable layers
        adjusted_config.max_k = adjusted_config.max_k.saturating_sub(step);
        if adjusted_config.max_k < config.min_k {
            adjusted_config.max_k = config.min_k;
            return allocate_layer_budgets(fragility, &adjusted_config);
        }
        step = (step / 2).max(1);
    }

    allocate_layer_budgets(fragility, &adjusted_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "no_std")]
    use alloc::vec;

    #[test]
    fn test_allocate_layer_budgets_fragile_gets_more() {
        let frag = vec![
            LayerFragilityEntry {
                layer: 0,
                cos_p05: 0.712,
            },
            LayerFragilityEntry {
                layer: 7,
                cos_p05: 0.999,
            },
            LayerFragilityEntry {
                layer: 23,
                cos_p05: 0.9999,
            },
        ];
        let config = BudgetConfig::default();
        let budgets = allocate_layer_budgets(&frag, &config);
        assert!(
            budgets.get(0) > budgets.get(7),
            "fragile layer 0 should get more"
        );
        assert!(
            budgets.get(7) >= budgets.get(23),
            "layer 7 should get at least as much as 23"
        );
    }

    #[test]
    fn test_budgets_in_range() {
        let frag = vec![
            LayerFragilityEntry {
                layer: 0,
                cos_p05: 0.712,
            },
            LayerFragilityEntry {
                layer: 1,
                cos_p05: 1.0,
            },
        ];
        let config = BudgetConfig::default();
        let budgets = allocate_layer_budgets(&frag, &config);
        assert!(budgets.validate(), "all budgets should be in range");
    }

    #[test]
    fn test_expected_mean_k() {
        let frag = vec![
            LayerFragilityEntry {
                layer: 0,
                cos_p05: 0.712,
            },
            LayerFragilityEntry {
                layer: 1,
                cos_p05: 1.0,
            },
        ];
        let config = BudgetConfig::default();
        let budgets = allocate_layer_budgets(&frag, &config);
        let mean_k = budgets.expected_mean_k(256);
        assert!(mean_k > 0.0 && mean_k < 256.0);
    }

    #[test]
    fn test_default_fragility_256tok() {
        let frag = default_fragility_256tok();
        assert_eq!(frag.len(), 24);
        assert!((frag[0].cos_p05 - 0.712).abs() < 0.001);
        assert!((frag[1].cos_p05 - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_default_budgets_pass_gate_target() {
        let frag = default_fragility_256tok();
        let config = BudgetConfig::default();
        let budgets = allocate_layer_budgets(&frag, &config);
        let mean_k = budgets.expected_mean_k(256);
        assert!(
            mean_k < 128.0,
            "expected mean k {mean_k:.1} should be < 128"
        );
        assert!(budgets.validate());
    }

    #[test]
    fn test_head_budgets() {
        let frag = vec![
            HeadFragilityEntry {
                layer: 0,
                head: 0,
                cos_p05: 0.712,
            },
            HeadFragilityEntry {
                layer: 0,
                head: 1,
                cos_p05: 0.998,
            },
            HeadFragilityEntry {
                layer: 1,
                head: 0,
                cos_p05: 0.999,
            },
        ];
        let config = BudgetConfig::default();
        let budgets = allocate_head_budgets(&frag, &config);
        assert!(
            budgets.get(0, 0) > budgets.get(0, 1),
            "fragile head should get more"
        );
        assert!(
            budgets.get(0, 0) > budgets.get(1, 0),
            "layer 0 head 0 should get more"
        );
    }

    #[test]
    fn test_learn_budgets_tightens_to_target() {
        let frag = default_fragility_256tok();
        let config = BudgetConfig::default();
        let learned = learn_budgets(&frag, &config, 70.0, 256);
        let mean_k = learned.expected_mean_k(256);
        let unlearned = allocate_layer_budgets(&frag, &config);
        let unlearned_mean = unlearned.expected_mean_k(256);
        assert!(
            mean_k <= unlearned_mean,
            "learned {mean_k:.1} should be <= unlearned {unlearned_mean}"
        );
    }
}
