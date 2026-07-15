//! RoPE-aware bit allocation for KV-cache quantization.
//!
//! Provides CPU-side, deterministic infrastructure for assigning variable
//! bit-widths to RoPE frequency blocks.  The greedy energy heuristic is
//! experimental and inspired by block-level frequency analysis; it does
//! **not** claim equivalence with any published algorithm.

/// A contiguous pair of head dimensions forming one RoPE frequency block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RopeBlock {
    /// Zero-based index of this block within the head.
    pub block_index: usize,
    /// Inclusive start dimension (always even).
    pub dim_start: usize,
    /// Exclusive end dimension (`dim_start + 2`).
    pub dim_end: usize,
}

/// Partition `head_dim` into contiguous 2-D RoPE blocks.
///
/// Each block covers `[2k, 2k+2)` for `k in 0..head_dim/2`.  When `head_dim`
/// is odd the single trailing dimension is silently ignored; only
/// `head_dim & !1` dimensions are covered.
pub fn rope_blocks(head_dim: usize) -> Vec<RopeBlock> {
    (0..head_dim / 2)
        .map(|k| RopeBlock {
            block_index: k,
            dim_start: 2 * k,
            dim_end: 2 * k + 2,
        })
        .collect()
}

/// Energy estimate for one RoPE block.
#[derive(Debug, Clone, Copy)]
pub struct RopeBlockEnergy {
    pub block: RopeBlock,
    /// Mean squared value across all key vectors that cover this block.
    pub energy: f32,
}

/// Compute per-block energy from a set of key vectors.
///
/// For each block covering `[dim_start, dim_end)`, only key vectors whose
/// length is `>= dim_end` contribute.  Vectors that are too short to reach a
/// block are silently skipped for that block.  If no vector covers a block its
/// energy is `0.0`.
pub fn rope_block_energies(keys: &[Vec<f32>], head_dim: usize) -> Vec<RopeBlockEnergy> {
    rope_blocks(head_dim)
        .into_iter()
        .map(|block| {
            let mut sum_sq = 0.0f64;
            let mut count = 0usize;
            for key in keys {
                if key.len() < block.dim_end {
                    continue;
                }
                for &v in &key[block.dim_start..block.dim_end] {
                    sum_sq += (v as f64) * (v as f64);
                }
                count += 1;
            }
            let energy = if count == 0 {
                0.0f32
            } else {
                (sum_sq / ((count * 2) as f64)) as f32
            };
            RopeBlockEnergy { block, energy }
        })
        .collect()
}

/// Bit-width assignment for every RoPE block in a head.
#[derive(Debug, Clone)]
pub struct RopeBitAllocation {
    /// Bit width assigned to block `i`.
    pub bits_per_block: Vec<u8>,
    /// Sum of `bits_per_block`.
    pub total_bits: usize,
}

/// Greedily allocate bits across RoPE blocks.
///
/// Initializes each block to `min_bits`, then spends the remaining budget one
/// bit at a time on the highest-energy block that has not yet reached
/// `max_bits`.  Equal-energy ties are broken by lower `block_index`.
///
/// Edge cases:
/// * If `total_bits < min_bits * n_blocks` the minimum floor is applied and
///   `RopeBitAllocation::total_bits` is set to `min_bits * n_blocks`.
/// * If `max_bits < min_bits`, `max_bits` is clamped up to `min_bits`; all
///   blocks receive exactly `min_bits`.
/// * An empty `energies` slice returns an empty allocation with `total_bits = 0`.
pub fn allocate_rope_bits(
    energies: &[RopeBlockEnergy],
    min_bits: u8,
    max_bits: u8,
    total_bits: usize,
) -> RopeBitAllocation {
    let n = energies.len();
    if n == 0 {
        return RopeBitAllocation {
            bits_per_block: vec![],
            total_bits: 0,
        };
    }

    let effective_max = max_bits.max(min_bits);
    let mut alloc: Vec<u8> = vec![min_bits; n];
    let min_total = (min_bits as usize) * n;

    if total_bits <= min_total {
        return RopeBitAllocation {
            bits_per_block: alloc,
            total_bits: min_total,
        };
    }

    let mut remaining = total_bits - min_total;
    while remaining > 0 {
        // Pick the eligible block with the highest energy; lower block_index
        // breaks ties deterministically.
        let best = energies
            .iter()
            .enumerate()
            .filter(|(i, _)| alloc[*i] < effective_max)
            .max_by(|(ia, a), (ib, b)| {
                a.energy
                    .partial_cmp(&b.energy)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(ib.cmp(ia)) // lower index wins on tie
            });
        match best {
            None => break, // all blocks at cap
            Some((idx, _)) => {
                alloc[idx] += 1;
                remaining -= 1;
            }
        }
    }

    let actual_total: usize = alloc.iter().map(|&b| b as usize).sum();
    RopeBitAllocation {
        bits_per_block: alloc,
        total_bits: actual_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rope_blocks_even_head_dim_returns_2d_spans() {
        let blocks = rope_blocks(8);
        assert_eq!(blocks.len(), 4);
        assert_eq!(
            blocks[0],
            RopeBlock {
                block_index: 0,
                dim_start: 0,
                dim_end: 2
            }
        );
        assert_eq!(
            blocks[1],
            RopeBlock {
                block_index: 1,
                dim_start: 2,
                dim_end: 4
            }
        );
        assert_eq!(
            blocks[2],
            RopeBlock {
                block_index: 2,
                dim_start: 4,
                dim_end: 6
            }
        );
        assert_eq!(
            blocks[3],
            RopeBlock {
                block_index: 3,
                dim_start: 6,
                dim_end: 8
            }
        );
    }

    #[test]
    fn rope_blocks_odd_head_dim_ignores_trailing_dim() {
        // head_dim=9 → 4 blocks; dimension index 8 is the trailing odd dim, not covered
        let blocks = rope_blocks(9);
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks.last().unwrap().dim_end, 8);
    }

    #[test]
    fn rope_block_energies_rank_high_energy_block_first() {
        // Block 1 ([2,4)) has energy 100, block 0 ([0,2)) is zero
        let keys = vec![vec![0.0f32, 0.0, 10.0, 10.0], vec![0.0f32, 0.0, 10.0, 10.0]];
        let energies = rope_block_energies(&keys, 4);
        assert_eq!(energies.len(), 2);
        assert!(energies[1].energy > energies[0].energy);
        assert_eq!(energies[0].energy, 0.0);
        assert!((energies[1].energy - 100.0f32).abs() < 1e-4);
    }

    #[test]
    fn rope_block_energies_empty_keys_are_zero() {
        let energies = rope_block_energies(&[], 8);
        assert_eq!(energies.len(), 4);
        for be in &energies {
            assert_eq!(be.energy, 0.0);
        }
    }

    #[test]
    fn allocate_rope_bits_honors_min_max_and_budget() {
        // 4 blocks, min=2 max=4, budget=12; base=8, 4 extra bits to spread
        let blocks = rope_blocks(8);
        let energies: Vec<RopeBlockEnergy> = blocks
            .iter()
            .map(|&block| RopeBlockEnergy { block, energy: 1.0 })
            .collect();
        let alloc = allocate_rope_bits(&energies, 2, 4, 12);
        assert_eq!(alloc.total_bits, 12);
        for &b in &alloc.bits_per_block {
            assert!((2..=4).contains(&b), "got {b}");
        }
    }

    #[test]
    fn allocate_rope_bits_prefers_high_energy_blocks() {
        let blocks = rope_blocks(8);
        let energies: Vec<RopeBlockEnergy> = blocks
            .iter()
            .enumerate()
            .map(|(i, &block)| RopeBlockEnergy {
                block,
                energy: if i == 3 { 100.0 } else { 1.0 },
            })
            .collect();
        // Exactly one extra bit above the min-total floor
        let alloc = allocate_rope_bits(&energies, 2, 8, 2 * 4 + 1);
        assert_eq!(
            alloc.bits_per_block[3], 3,
            "extra bit must go to highest-energy block"
        );
        assert_eq!(alloc.bits_per_block[0], 2);
    }

    #[test]
    fn allocate_rope_bits_tie_breaks_by_block_index() {
        let blocks = rope_blocks(8);
        // All blocks have identical energy
        let energies: Vec<RopeBlockEnergy> = blocks
            .iter()
            .map(|&block| RopeBlockEnergy { block, energy: 5.0 })
            .collect();
        // Exactly one extra bit; must land on block 0 (lowest index)
        let alloc = allocate_rope_bits(&energies, 2, 8, 2 * 4 + 1);
        assert_eq!(
            alloc.bits_per_block[0], 3,
            "tie must break to lower block_index"
        );
        assert_eq!(alloc.bits_per_block[1], 2);
        assert_eq!(alloc.bits_per_block[2], 2);
        assert_eq!(alloc.bits_per_block[3], 2);
    }
}
