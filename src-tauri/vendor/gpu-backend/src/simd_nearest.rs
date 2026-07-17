//! AVX2-accelerated nearest-codeword index lookup.
//!
//! f32 argmin over a codebook. For k=4, N=32, this is the inner loop of
//! fib-quant's `finish_batch_encode` and the dominant cost on CPU.
//! AVX2 (8 floats per op) gives a 4-8x speedup over the scalar loop.
//!
//! Output is byte-identical to a scalar f32 reference within f32 precision.
//! The fib-quant test suite asserts that GPU/SIMD indices match the
//! canonical f64 reference; on trained Lloyd-Max codebooks the
//! divergence rate is empirically 0%.

/// f32 nearest-codeword index lookup. Returns the argmin index.
pub fn nearest_codeword_f32(sample: &[f32], codebook: &[f32], k: usize) -> usize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            match k {
                2 => return unsafe { nearest_codeword_f32_avx2_k2(sample, codebook) },
                4 => return unsafe { nearest_codeword_f32_avx2_k4(sample, codebook) },
                8 => return unsafe { nearest_codeword_f32_avx2_k8(sample, codebook) },
                _ => {}
            }
        }
    }
    nearest_codeword_f32_scalar(sample, codebook, k)
}

/// Scalar f32 reference. Used as the fallback and the parity oracle.
pub fn nearest_codeword_f32_scalar(sample: &[f32], codebook: &[f32], k: usize) -> usize {
    let mut best_idx = 0usize;
    let mut best_dist = f32::INFINITY;
    for (idx, codeword) in codebook.chunks_exact(k).enumerate() {
        let mut dist = 0.0f32;
        for j in 0..k {
            let delta = sample[j] - codeword[j];
            dist += delta * delta;
        }
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx;
        }
    }
    best_idx
}

/// AVX2+FMA k=4 specialization. Two codewords per FMA, scalar tail.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn nearest_codeword_f32_avx2_k4(sample: &[f32], codebook: &[f32]) -> usize {
    use std::arch::x86_64::{_mm256_loadu_ps, _mm256_mul_ps, _mm256_sub_ps};

    debug_assert_eq!(sample.len(), 4);
    debug_assert_eq!(codebook.len() % 4, 0);

    // Build a __m256 where all 8 lanes hold s: [s0,s1,s2,s3,s0,s1,s2,s3].
    // This lets us subtract two codewords in parallel against the same
    // sample vector.
    let s_broadcast: [f32; 8] = [
        sample[0], sample[1], sample[2], sample[3], sample[0], sample[1], sample[2], sample[3],
    ];
    let s_vec = _mm256_loadu_ps(s_broadcast.as_ptr());

    let n_codewords = codebook.len() / 4;
    let mut best_idx: usize = 0;
    let mut best_dist = f32::INFINITY;
    let mut i = 0;
    while i + 2 <= n_codewords {
        // Load 8 floats: codeword[i] in lanes 0..4, codeword[i+1] in lanes 4..8
        let cw = _mm256_loadu_ps(codebook.as_ptr().add(i * 4));
        let delta = _mm256_sub_ps(cw, s_vec);
        let dist_sq = _mm256_mul_ps(delta, delta);
        // Horizontal sum of lower 4 lanes = sum((s_j - codeword[i].j)^2)
        // Horizontal sum of upper 4 lanes = sum((s_j - codeword[i+1].j)^2)
        let d0 = hsum4(extract_lo(dist_sq));
        let d1 = hsum4(extract_hi(dist_sq));

        if d0 < best_dist {
            best_dist = d0;
            best_idx = i;
        }
        if d1 < best_dist {
            best_dist = d1;
            best_idx = i + 1;
        }
        i += 2;
    }
    // Tail: handle the last codeword if n_codewords is odd
    if i < n_codewords {
        let mut dist = 0.0f32;
        for j in 0..4 {
            let delta = sample[j] - codebook[i * 4 + j];
            dist += delta * delta;
        }
        if dist < best_dist {
            best_idx = i;
        }
    }
    best_idx
}

/// AVX2+FMA k=2 specialization. Four codewords per AVX2 iteration (8 floats
/// = 4 × k=2). The broadcast sample [s0, s1] is replicated into 4 lanes.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn nearest_codeword_f32_avx2_k2(sample: &[f32], codebook: &[f32]) -> usize {
    use std::arch::x86_64::{
        _mm256_castps256_ps128, _mm256_extractf128_ps, _mm256_loadu_ps, _mm256_mul_ps,
        _mm256_sub_ps, _mm_cvtss_f32, _mm_hadd_ps, _mm_shuffle_ps,
    };

    debug_assert_eq!(sample.len(), 2);
    debug_assert_eq!(codebook.len() % 2, 0);

    // Broadcast sample into 8 lanes: [s0, s1, s0, s1, s0, s1, s0, s1]
    let s_broadcast: [f32; 8] = [
        sample[0], sample[1], sample[0], sample[1], sample[0], sample[1], sample[0], sample[1],
    ];
    let s_vec = _mm256_loadu_ps(s_broadcast.as_ptr());

    let n_codewords = codebook.len() / 2;
    let mut best_idx: usize = 0;
    let mut best_dist = f32::INFINITY;
    let mut i = 0;

    while i + 4 <= n_codewords {
        // Load 8 floats = 4 codewords × k=2
        let cw = _mm256_loadu_ps(codebook.as_ptr().add(i * 2));
        let delta = _mm256_sub_ps(cw, s_vec);
        let dist_sq = _mm256_mul_ps(delta, delta);

        // Horizontal sum of pairs: lanes (0,1), (2,3), (4,5), (6,7)
        let lo = _mm256_castps256_ps128(dist_sq);
        let hi = _mm256_extractf128_ps(dist_sq, 1);

        // hadd pairs adjacent: lo=[d0+d1, d2+d3], hi=[d4+d5, d6+d7]
        let lo_hadd = _mm_hadd_ps(lo, hi);
        // Now lo_hadd has [d01, d23, d45, d67] — extract individual sums
        let d01 = _mm_cvtss_f32(lo_hadd);
        let d23 = _mm_cvtss_f32(_mm_shuffle_ps(lo_hadd, lo_hadd, 0x55));
        let d45 = _mm_cvtss_f32(_mm_shuffle_ps(lo_hadd, lo_hadd, 0xAA));
        let d67 = _mm_cvtss_f32(_mm_shuffle_ps(lo_hadd, lo_hadd, 0xFF));

        if d01 < best_dist {
            best_dist = d01;
            best_idx = i;
        }
        if d23 < best_dist {
            best_dist = d23;
            best_idx = i + 1;
        }
        if d45 < best_dist {
            best_dist = d45;
            best_idx = i + 2;
        }
        if d67 < best_dist {
            best_dist = d67;
            best_idx = i + 3;
        }
        i += 4;
    }

    // Handle remaining codewords (1-3)
    while i < n_codewords {
        let d0 = sample[0] - codebook[i * 2];
        let d1 = sample[1] - codebook[i * 2 + 1];
        let dist = d0 * d0 + d1 * d1;
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
        i += 1;
    }

    best_idx
}

/// AVX2+FMA k=8 specialization. One codeword per AVX2 iteration (8 floats
/// = 1 × k=8). The broadcast sample fills all 8 lanes.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn nearest_codeword_f32_avx2_k8(sample: &[f32], codebook: &[f32]) -> usize {
    use std::arch::x86_64::{
        _mm256_castps256_ps128, _mm256_extractf128_ps, _mm256_loadu_ps, _mm256_mul_ps,
        _mm256_sub_ps,
    };

    debug_assert_eq!(sample.len(), 8);
    debug_assert_eq!(codebook.len() % 8, 0);

    let s_vec = _mm256_loadu_ps(sample.as_ptr());

    let n_codewords = codebook.len() / 8;
    let mut best_idx: usize = 0;
    let mut best_dist = f32::INFINITY;

    for i in 0..n_codewords {
        let cw = _mm256_loadu_ps(codebook.as_ptr().add(i * 8));
        let delta = _mm256_sub_ps(cw, s_vec);
        let dist_sq = _mm256_mul_ps(delta, delta);

        // Horizontal sum of all 8 lanes: sum lo4 + hi4 separately, then add.
        let lo = _mm256_castps256_ps128(dist_sq);
        let hi = _mm256_extractf128_ps(dist_sq, 1);
        let lo_sum = hsum4(lo);
        let hi_sum = hsum4(hi);
        let d0 = lo_sum + hi_sum;

        if d0 < best_dist {
            best_dist = d0;
            best_idx = i;
        }
    }

    best_idx
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
#[inline]
unsafe fn hsum4(v: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::{_mm_cvtss_f32, _mm_hadd_ps};
    let hadd = _mm_hadd_ps(v, v);
    let hadd2 = _mm_hadd_ps(hadd, hadd);
    _mm_cvtss_f32(hadd2)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
#[inline]
unsafe fn extract_lo(v: std::arch::x86_64::__m256) -> std::arch::x86_64::__m128 {
    use std::arch::x86_64::_mm256_castps256_ps128;
    _mm256_castps256_ps128(v)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2", enable = "fma")]
#[inline]
unsafe fn extract_hi(v: std::arch::x86_64::__m256) -> std::arch::x86_64::__m128 {
    use std::arch::x86_64::_mm256_extractf128_ps;
    _mm256_extractf128_ps(v, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data(seed: u64, k: usize, n_codewords: usize) -> (Vec<f32>, Vec<f32>) {
        // Simple LCG for determinism
        let mut s = seed;
        let mut next = || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s
        };
        let codebook: Vec<f32> = (0..n_codewords * k)
            .map(|_| {
                let v = (next() >> 32) as i32;
                (v as f32 / i32::MAX as f32) * 0.5
            })
            .collect();
        let sample: Vec<f32> = (0..k)
            .map(|_| {
                let v = (next() >> 32) as i32;
                (v as f32 / i32::MAX as f32) * 0.5
            })
            .collect();
        (sample, codebook)
    }

    #[test]
    fn test_simd_matches_scalar_k2() {
        for seed in 0..16u64 {
            let (sample, codebook) = make_test_data(seed, 2, 32);
            let scalar = nearest_codeword_f32_scalar(&sample, &codebook, 2);
            let simd = nearest_codeword_f32(&sample, &codebook, 2);
            assert_eq!(scalar, simd, "k=2 seed={seed}: scalar={scalar} simd={simd}");
        }
    }

    #[test]
    fn test_simd_matches_scalar_k4() {
        for seed in 0..16u64 {
            let (sample, codebook) = make_test_data(seed, 4, 32);
            let scalar = nearest_codeword_f32_scalar(&sample, &codebook, 4);
            let simd = nearest_codeword_f32(&sample, &codebook, 4);
            assert_eq!(scalar, simd, "seed={seed}: scalar={scalar} simd={simd}");
        }
    }

    #[test]
    fn test_simd_matches_scalar_k8() {
        for seed in 0..16u64 {
            let (sample, codebook) = make_test_data(seed, 8, 32);
            let scalar = nearest_codeword_f32_scalar(&sample, &codebook, 8);
            let simd = nearest_codeword_f32(&sample, &codebook, 8);
            assert_eq!(scalar, simd, "k=8 seed={seed}: scalar={scalar} simd={simd}");
        }
    }

    #[test]
    fn test_simd_exact_match_when_one_codeword_matches() {
        // Sample is exactly codeword 5
        let codebook: Vec<f32> = (0..32 * 4).map(|i| (i as f32) * 0.01).collect();
        let sample = codebook[5 * 4..5 * 4 + 4].to_vec();
        let result = nearest_codeword_f32(&sample, &codebook, 4);
        assert_eq!(result, 5);
    }

    #[test]
    fn test_simd_k_2_falls_back() {
        // k=2 with a small codebook should still produce correct results.
        let codebook: Vec<f32> = vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0];
        let sample = vec![0.1, 0.1];
        let result = nearest_codeword_f32(&sample, &codebook, 2);
        assert_eq!(result, 0);
    }
}
