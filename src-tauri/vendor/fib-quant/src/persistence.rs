//! Disk persistence and zero-copy mmap access for [`FibSidecarIndex`].
//!
//! This module provides a binary file format (`FBS1`) for serializing a
//! [`FibSidecarIndex`] to disk and two loading modes:
//!
//! - [`load_from_file`] — fully owned reconstruction (reads everything into
//!   `Vec`s, decodes compact bytes to `FibCodeV1`).
//! - [`load_mmap`] — zero-copy memory-mapped access. Compressed payloads
//!   stay in the mmap as `&[u8]` slices and are decoded on demand.
//!
//! ## File Format (`FBS1`)
//!
//! ```text
//! [0..4]   magic: "FBS1"
//! [4]      version: u8 (1)
//! [5..9]   profile_json_len: u32
//! [9..9+L] profile_json: JSON-serialized FibQuantProfileV1
//! [9+L..13+L] gram_len: u32 (number of f32 values = N*N)
//! [13+L..13+L+G] gram_values: N*N f32 (little-endian, row-major)
//! [13+L+G..17+L+G] entry_count: u32
//! Per entry:
//!   id_len: u32 (little-endian)
//!   id_bytes: id_len bytes (postcard-serialized Id)
//!   code_len: u32 (little-endian)
//!   code_bytes: code_len bytes (FibCodeV1::to_compact_bytes)
//! ```

use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::{
    codec::{FibCodeV1, FibQuantizer},
    profile::FibQuantProfileV1,
    scoring::FibScorer,
    sidecar::FibSidecarIndex,
    FibQuantError, Result,
};

#[cfg(feature = "mmap")]
use crate::sidecar::ScoredCandidate;

// ──────────────────────────────────────────────────────────────────────
//  Constants
// ──────────────────────────────────────────────────────────────────────

/// Magic bytes: "FBS1" = Fib Sidecar v1.
pub const FILE_MAGIC: [u8; 4] = *b"FBS1";

/// Current file format version.
pub const FILE_VERSION: u8 = 1;

/// Minimum header size before variable-length data: magic(4) + version(1)
/// = 5 bytes. The profile JSON length prefix (u32) is read as part of
/// the first `read_len_prefixed` call, not as part of the header.
const MIN_HEADER_SIZE: usize = 5;

// ──────────────────────────────────────────────────────────────────────
//  On-disk format struct (for documentation; the actual serialization
//  is hand-rolled for precise layout control).
// ──────────────────────────────────────────────────────────────────────

/// On-disk file format for [`FibSidecarIndex`].
///
/// This struct is not directly serialized — the binary layout is written
/// manually by [`save_to_file`] for precise control. It serves as
/// documentation of the format.
#[derive(Debug)]
pub struct FibSidecarFileV1 {
    /// Magic: "FBS1".
    pub magic: [u8; 4],
    /// Format version (currently 1).
    pub version: u8,
    /// Serialized profile (JSON).
    pub profile: FibQuantProfileV1,
    /// Gram table values (N*N f32, row-major).
    pub gram_values: Vec<f32>,
    /// Number of entries.
    pub entry_count: u32,
    // Per entry: ID (length-prefixed bytes) + FibCodeV1 compact bytes
}

// ──────────────────────────────────────────────────────────────────────
//  Writer helpers
// ──────────────────────────────────────────────────────────────────────

/// Write a u32 length prefix (little-endian) followed by data.
fn write_len_prefixed(out: &mut Vec<u8>, data: &[u8]) {
    let len = u32::try_from(data.len()).expect("length fits in u32");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(data);
}

// ──────────────────────────────────────────────────────────────────────
//  Reader helpers
// ──────────────────────────────────────────────────────────────────────

/// Read a u32 length prefix from `buf` at `offset`. Returns
/// `(length, new_offset)`.
fn read_u32_at(buf: &[u8], offset: usize, label: &str) -> Result<(u32, usize)> {
    if offset + 4 > buf.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "truncated file: cannot read {} u32 at offset {} (file len {})",
            label,
            offset,
            buf.len()
        )));
    }
    let val = u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]);
    Ok((val, offset + 4))
}

/// Read a length-prefixed slice from `buf` at `offset`. Returns
/// `(slice, new_offset)`.
fn read_len_prefixed<'a>(buf: &'a [u8], offset: usize, label: &str) -> Result<(&'a [u8], usize)> {
    let (len, next) = read_u32_at(buf, offset, &format!("{} length", label))?;
    let len = len as usize;
    if next + len > buf.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "truncated file: {} data at offset {} needs {} bytes but only {} remain",
            label,
            next,
            len,
            buf.len() - next
        )));
    }
    Ok((&buf[next..next + len], next + len))
}

/// Read `count` f32 values (little-endian) from `buf` at `offset`.
fn read_f32_slice(
    buf: &[u8],
    offset: usize,
    count: usize,
    label: &str,
) -> Result<(Vec<f32>, usize)> {
    let byte_len = count.checked_mul(4).ok_or_else(|| {
        FibQuantError::ResourceLimitExceeded(format!("{} f32 count overflow", label))
    })?;
    if offset + byte_len > buf.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "truncated file: {} needs {} bytes at offset {} but only {} remain",
            label,
            byte_len,
            offset,
            buf.len() - offset
        )));
    }
    let mut values = Vec::with_capacity(count);
    for i in 0..count {
        let base = offset + i * 4;
        values.push(f32::from_le_bytes([
            buf[base],
            buf[base + 1],
            buf[base + 2],
            buf[base + 3],
        ]));
    }
    Ok((values, offset + byte_len))
}

// ──────────────────────────────────────────────────────────────────────
//  save_to_file
// ──────────────────────────────────────────────────────────────────────

/// Serialize a [`FibSidecarIndex`] to a binary file.
///
/// The file format is `FBS1` (see module docs for layout). The profile
/// is serialized as JSON, the Gram table as raw f32 values, and each
/// entry's `FibCodeV1` as compact bytes (`to_compact_bytes`).
///
/// The `Id` type must implement `Serialize` (via `postcard`).
///
/// # Errors
///
/// Returns [`FibQuantError::CorruptPayload`] if serialization fails.
pub fn save_to_file<Id>(index: &FibSidecarIndex<Id>, path: &Path) -> Result<()>
where
    Id: Clone + Eq + std::fmt::Debug + Serialize,
{
    let bytes = serialize_index(index)?;
    std::fs::write(path, &bytes).map_err(|e| {
        FibQuantError::CorruptPayload(format!("failed to write sidecar file {:?}: {}", path, e))
    })
}

/// Serialize an index to bytes (the full `FBS1` file content).
fn serialize_index<Id>(index: &FibSidecarIndex<Id>) -> Result<Vec<u8>>
where
    Id: Clone + Eq + std::fmt::Debug + Serialize,
{
    // Profile → JSON
    let profile = index.profile();
    let profile_json = serde_json::to_vec(profile).map_err(|e| {
        FibQuantError::CorruptPayload(format!("failed to serialize profile as JSON: {}", e))
    })?;

    // Gram table values
    let gram_values = index.scorer().gram_table().values();
    let gram_count = gram_values.len();

    // Entries
    let entries = index.entries();
    let entry_count = u32::try_from(entries.len())
        .map_err(|_| FibQuantError::ResourceLimitExceeded("entry count exceeds u32".into()))?;

    // Estimate total size for pre-allocation
    let estimated_size =
        MIN_HEADER_SIZE + 4 + profile_json.len() + 4 + gram_count * 4 + 4 + entries.len() * 40; // rough estimate per entry
    let mut out = Vec::with_capacity(estimated_size);

    // Header
    out.extend_from_slice(&FILE_MAGIC);
    out.push(FILE_VERSION);

    // Profile JSON (length-prefixed)
    write_len_prefixed(&mut out, &profile_json);

    // Gram table: u32 count + f32 values
    let gram_u32 = u32::try_from(gram_count)
        .map_err(|_| FibQuantError::ResourceLimitExceeded("gram table size exceeds u32".into()))?;
    out.extend_from_slice(&gram_u32.to_le_bytes());
    for &v in gram_values {
        out.extend_from_slice(&v.to_le_bytes());
    }

    // Entry count
    out.extend_from_slice(&entry_count.to_le_bytes());

    // Per entry: id (postcard, length-prefixed) + code compact bytes (length-prefixed)
    for (id, code) in entries {
        let id_bytes = postcard::to_stdvec(id).map_err(|e| {
            FibQuantError::CorruptPayload(format!("failed to serialize entry id: {}", e))
        })?;
        write_len_prefixed(&mut out, &id_bytes);
        let code_bytes = code.to_compact_bytes();
        write_len_prefixed(&mut out, &code_bytes);
    }

    Ok(out)
}

// ──────────────────────────────────────────────────────────────────────
//  load_from_file
// ──────────────────────────────────────────────────────────────────────

/// Load a [`FibSidecarIndex`] from a binary file (fully owned).
///
/// Reads the file, reconstructs the [`FibScorer`] from the profile (by
/// rebuilding the quantizer), and decodes each entry's compact bytes
/// into a [`FibCodeV1`].
///
/// The `Id` type must implement `DeserializeOwned` (via `postcard`).
///
/// # Errors
///
/// Returns [`FibQuantError::CorruptPayload`] for malformed files or
/// deserialization failures.
pub fn load_from_file<Id>(path: &Path) -> Result<FibSidecarIndex<Id>>
where
    Id: Clone + Eq + std::fmt::Debug + DeserializeOwned,
{
    let bytes = std::fs::read(path).map_err(|e| {
        FibQuantError::CorruptPayload(format!("failed to read sidecar file {:?}: {}", path, e))
    })?;
    deserialize_index(&bytes)
}

/// Deserialize an index from bytes (the full `FBS1` file content).
fn deserialize_index<Id>(buf: &[u8]) -> Result<FibSidecarIndex<Id>>
where
    Id: Clone + Eq + std::fmt::Debug + DeserializeOwned,
{
    // Header
    if buf.len() < MIN_HEADER_SIZE {
        return Err(FibQuantError::CorruptPayload(format!(
            "file too short: {} bytes (need >= {})",
            buf.len(),
            MIN_HEADER_SIZE
        )));
    }
    if buf[0..4] != FILE_MAGIC {
        return Err(FibQuantError::CorruptPayload(format!(
            "bad file magic: {:?} (expected {:?})",
            &buf[0..4],
            FILE_MAGIC
        )));
    }
    if buf[4] != FILE_VERSION {
        return Err(FibQuantError::CorruptPayload(format!(
            "unsupported file version {} (expected {})",
            buf[4], FILE_VERSION
        )));
    }

    let mut offset = MIN_HEADER_SIZE; // after magic(4) + version(1)

    // Profile JSON (length-prefixed)
    let (profile_bytes, next) = read_len_prefixed(buf, offset, "profile")?;
    offset = next;
    let profile: FibQuantProfileV1 = serde_json::from_slice(profile_bytes).map_err(|e| {
        FibQuantError::CorruptPayload(format!("failed to deserialize profile JSON: {}", e))
    })?;

    // Gram table: u32 count + f32 values
    let (gram_count, next) = read_u32_at(buf, offset, "gram count")?;
    offset = next;
    let gram_count = gram_count as usize;
    let (gram_values, next) = read_f32_slice(buf, offset, gram_count, "gram table")?;
    offset = next;

    // Entry count
    let (entry_count, next) = read_u32_at(buf, offset, "entry count")?;
    offset = next;
    let entry_count = entry_count as usize;

    // Reconstruct the scorer. We rebuild the quantizer from the profile
    // (which rebuilds the codebook and rotation deterministically from
    // the seed), then construct a FibScorer. The Gram table we read from
    // the file is validated against the reconstructed one.
    let quantizer = FibQuantizer::new(profile.clone())?;
    let scorer = FibScorer::new(quantizer)?;

    // Validate gram table matches the reconstructed one
    let reconstructed_gram = scorer.gram_table().values();
    if reconstructed_gram.len() != gram_values.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "gram table size mismatch: file has {} values, reconstructed has {}",
            gram_values.len(),
            reconstructed_gram.len()
        )));
    }
    for (i, (a, b)) in gram_values
        .iter()
        .zip(reconstructed_gram.iter())
        .enumerate()
    {
        if (a - b).abs() > 1e-5 {
            return Err(FibQuantError::CorruptPayload(format!(
                "gram table value mismatch at index {}: file={}, reconstructed={}",
                i, a, b
            )));
        }
    }

    // Reconstruct index
    let mut index = FibSidecarIndex::<Id>::new(scorer);

    // Read entries
    for i in 0..entry_count {
        // ID (length-prefixed postcard)
        let (id_bytes, next) = read_len_prefixed(buf, offset, &format!("entry {} id", i))?;
        offset = next;
        let id: Id = postcard::from_bytes(id_bytes).map_err(|e| {
            FibQuantError::CorruptPayload(format!("failed to deserialize entry {} id: {}", i, e))
        })?;

        // Code (length-prefixed compact bytes)
        let (code_bytes, next) = read_len_prefixed(buf, offset, &format!("entry {} code", i))?;
        offset = next;
        let code = FibCodeV1::from_compact_bytes(code_bytes, &profile)?;
        index.add(id, code);
    }

    Ok(index)
}

// ──────────────────────────────────────────────────────────────────────
//  MmapSidecarIndex (zero-copy mmap access) — requires "mmap" feature
// ──────────────────────────────────────────────────────────────────────

/// A memory-mapped sidecar index with zero-copy access to compressed
/// payloads.
///
/// The file is memory-mapped via [`memmap2::Mmap`]. The [`FibScorer`] is
/// reconstructed from the profile and gram table in the file (owned
/// data). Each entry's compressed `FibCodeV1` bytes remain as `&[u8]`
/// slices into the mmap — they are decoded on demand during search.
///
/// The `_mmap` field keeps the mapping alive for the lifetime of this
/// struct.
#[cfg(feature = "mmap")]
pub struct MmapSidecarIndex<Id>
where
    Id: Clone + Eq + std::fmt::Debug,
{
    /// Reconstructed scorer (owned).
    scorer: FibScorer,
    /// Entries: deserialized ID + zero-copy compressed payload slice.
    entries: Vec<(Id, &'static [u8])>,
    /// The profile (owned, for decoding compact bytes on demand).
    profile: FibQuantProfileV1,
    /// The underlying mmap, kept alive so slices in `entries` remain valid.
    _mmap: memmap2::Mmap,
}

#[cfg(feature = "mmap")]
impl<Id> MmapSidecarIndex<Id>
where
    Id: Clone + Eq + std::fmt::Debug + DeserializeOwned,
{
    /// Memory-map a sidecar file and provide zero-copy access to
    /// compressed payloads.
    ///
    /// The [`FibScorer`] is reconstructed from the profile (by rebuilding
    /// the quantizer). Each entry's compact bytes stay as zero-copy
    /// `&[u8]` slices into the mmap and are decoded on demand during
    /// search.
    ///
    /// # Safety
    ///
    /// This uses `Mmap::map()` which requires the file to be a valid
    /// mmap-able file. The file must not be modified while the mmap is
    /// alive.
    ///
    /// # Errors
    ///
    /// Returns [`FibQuantError::CorruptPayload`] for malformed files,
    /// or IO errors wrapped in `CorruptPayload`.
    #[allow(unsafe_code)]
    pub fn open(path: &Path) -> Result<Self> {
        // Open the file
        let file = std::fs::File::open(path).map_err(|e| {
            FibQuantError::CorruptPayload(format!("failed to open sidecar file {:?}: {}", path, e))
        })?;

        // Memory-map the file
        let mmap = unsafe {
            memmap2::Mmap::map(&file).map_err(|e| {
                FibQuantError::CorruptPayload(format!(
                    "failed to mmap sidecar file {:?}: {}",
                    path, e
                ))
            })?
        };
        let buf = &mmap[..];

        // Parse header
        if buf.len() < MIN_HEADER_SIZE {
            return Err(FibQuantError::CorruptPayload(format!(
                "file too short: {} bytes (need >= {})",
                buf.len(),
                MIN_HEADER_SIZE
            )));
        }
        if buf[0..4] != FILE_MAGIC {
            return Err(FibQuantError::CorruptPayload(format!(
                "bad file magic: {:?} (expected {:?})",
                &buf[0..4],
                FILE_MAGIC
            )));
        }
        if buf[4] != FILE_VERSION {
            return Err(FibQuantError::CorruptPayload(format!(
                "unsupported file version {} (expected {})",
                buf[4], FILE_VERSION
            )));
        }

        let mut offset = MIN_HEADER_SIZE;

        // Profile JSON
        let (profile_bytes, next) = read_len_prefixed(buf, offset, "profile")?;
        offset = next;
        let profile: FibQuantProfileV1 = serde_json::from_slice(profile_bytes).map_err(|e| {
            FibQuantError::CorruptPayload(format!("failed to deserialize profile JSON: {}", e))
        })?;

        // Gram table
        let (gram_count, next) = read_u32_at(buf, offset, "gram count")?;
        offset = next;
        let gram_count = gram_count as usize;
        let (gram_values, next) = read_f32_slice(buf, offset, gram_count, "gram table")?;
        offset = next;

        // Entry count
        let (entry_count, next) = read_u32_at(buf, offset, "entry count")?;
        offset = next;
        let entry_count = entry_count as usize;

        // Reconstruct the scorer
        let quantizer = FibQuantizer::new(profile.clone())?;
        let scorer = FibScorer::new(quantizer)?;

        // Validate gram table
        let reconstructed_gram = scorer.gram_table().values();
        if reconstructed_gram.len() != gram_values.len() {
            return Err(FibQuantError::CorruptPayload(format!(
                "gram table size mismatch: file has {} values, reconstructed has {}",
                gram_values.len(),
                reconstructed_gram.len()
            )));
        }
        for (i, (a, b)) in gram_values
            .iter()
            .zip(reconstructed_gram.iter())
            .enumerate()
        {
            if (a - b).abs() > 1e-5 {
                return Err(FibQuantError::CorruptPayload(format!(
                    "gram table value mismatch at index {}: file={}, reconstructed={}",
                    i, a, b
                )));
            }
        }

        // Read entries — decode ID from the mmap slice, keep code bytes
        // as zero-copy slices.
        let mut entries: Vec<(Id, &'static [u8])> = Vec::with_capacity(entry_count);
        for i in 0..entry_count {
            // ID (length-prefixed postcard)
            let (id_bytes, next) = read_len_prefixed(buf, offset, &format!("entry {} id", i))?;
            offset = next;
            let id: Id = postcard::from_bytes(id_bytes).map_err(|e| {
                FibQuantError::CorruptPayload(format!(
                    "failed to deserialize entry {} id: {}",
                    i, e
                ))
            })?;

            // Code (length-prefixed compact bytes) — keep as zero-copy slice
            let (code_bytes, next) = read_len_prefixed(buf, offset, &format!("entry {} code", i))?;
            offset = next;

            // SAFETY: The mmap is kept alive in self._mmap. The slice
            // borrows from the mmap's memory. We extend the lifetime to
            // 'static because the MmapSidecarIndex owns the Mmap and
            // the slices will never outlive it. This is the standard
            // pattern for mmap-backed zero-copy structures.
            let code_slice: &'static [u8] =
                unsafe { std::slice::from_raw_parts(code_bytes.as_ptr(), code_bytes.len()) };
            entries.push((id, code_slice));
        }

        // Drop the gram_values vec — we only needed it for validation.
        drop(gram_values);

        Ok(Self {
            scorer,
            entries,
            profile,
            _mmap: mmap,
        })
    }

    /// Return a reference to the scorer.
    pub fn scorer(&self) -> &FibScorer {
        &self.scorer
    }

    /// Return a reference to the profile.
    pub fn profile(&self) -> &FibQuantProfileV1 {
        &self.profile
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Approximate search returning sorted candidates.
    ///
    /// Decodes each entry's compressed bytes on demand from the mmap
    /// slices, scores them against the query using the Gram table
    /// estimator, and returns the top `top_k * oversample` candidates.
    ///
    /// See [`FibSidecarIndex::search`] for full semantics.
    pub fn search(
        &self,
        query: &[f32],
        top_k: usize,
        oversample: usize,
    ) -> Result<Vec<ScoredCandidate<Id>>>
    where
        Id: Clone,
    {
        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.entries.len());
        for (idx, (_, code_bytes)) in self.entries.iter().enumerate() {
            let code = FibCodeV1::from_compact_bytes(code_bytes, &self.profile)?;
            let s = self.scorer.inner_product_estimate(query, &code)?;
            scored.push((idx, s));
        }
        // Sort descending by score, stable for insertion-order tie-breaking.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let limit = top_k.saturating_mul(oversample.max(1)).min(scored.len());
        let candidates = scored
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(rank, (idx, score))| {
                let id = self.entries[idx].0.clone();
                ScoredCandidate {
                    id,
                    approximate_score: score,
                    rank,
                }
            })
            .collect();
        Ok(candidates)
    }
}

/// Load a sidecar index via memory-mapping (zero-copy).
///
/// Convenience wrapper around [`MmapSidecarIndex::open`].
#[cfg(feature = "mmap")]
pub fn load_mmap<Id>(path: &Path) -> Result<MmapSidecarIndex<Id>>
where
    Id: Clone + Eq + std::fmt::Debug + DeserializeOwned,
{
    MmapSidecarIndex::open(path)
}

// ──────────────────────────────────────────────────────────────────────
//  Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FibQuantProfileV1;
    use crate::{FibQuantizer, FibScorer};
    use tempfile::tempdir;

    fn build_test_scorer() -> Result<FibScorer> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        let quantizer = FibQuantizer::new(profile)?;
        FibScorer::new(quantizer)
    }

    fn make_vectors(d: usize, count: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|seed| {
                (0..d)
                    .map(|i| (seed as f32 * 0.1 + i as f32 * 0.05 - 0.3).sin())
                    .collect()
            })
            .collect()
    }

    fn build_test_index() -> Result<FibSidecarIndex<u32>> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);
        let vectors = make_vectors(d, 16);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }
        Ok(index)
    }

    #[test]
    fn save_and_load_produces_identical_search_results() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("index.fbs1");

        let original = build_test_index()?;
        save_to_file(&original, &path)?;
        let loaded: FibSidecarIndex<u32> = load_from_file(&path)?;

        assert_eq!(loaded.len(), original.len());

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let original_results = original.search(&query, 5, 2)?;
        let loaded_results = loaded.search(&query, 5, 2)?;

        assert_eq!(loaded_results.len(), original_results.len());
        for (orig, load) in original_results.iter().zip(loaded_results.iter()) {
            assert_eq!(orig.id, load.id, "id mismatch at rank {}", orig.rank);
            assert_eq!(orig.rank, load.rank, "rank mismatch");
            assert!(
                (orig.approximate_score - load.approximate_score).abs() < 1e-5,
                "score mismatch at rank {}: {} vs {}",
                orig.rank,
                orig.approximate_score,
                load.approximate_score
            );
        }
        Ok(())
    }

    #[test]
    fn file_magic_detection() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("magic_test.fbs1");

        let index = build_test_index()?;
        save_to_file(&index, &path)?;

        let bytes = std::fs::read(&path)
            .map_err(|e| FibQuantError::CorruptPayload(format!("read failed: {}", e)))?;

        // Check magic
        assert_eq!(&bytes[0..4], b"FBS1", "file should start with FBS1 magic");
        assert_eq!(bytes[4], FILE_VERSION, "version byte should be 1");

        // Bad magic should fail
        let mut bad = bytes.clone();
        bad[0] = b'X';
        let result: Result<FibSidecarIndex<u32>> = deserialize_index(&bad);
        assert!(result.is_err(), "bad magic should cause load failure");

        Ok(())
    }

    #[test]
    fn truncation_rejection() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("trunc_test.fbs1");

        let index = build_test_index()?;
        save_to_file(&index, &path)?;

        let bytes = std::fs::read(&path)
            .map_err(|e| FibQuantError::CorruptPayload(format!("read failed: {}", e)))?;

        // Truncate to just header — should fail
        let truncated = &bytes[..MIN_HEADER_SIZE + 2];
        let result: Result<FibSidecarIndex<u32>> = deserialize_index(truncated);
        assert!(result.is_err(), "truncated file should be rejected");

        // Empty file should fail
        let result: Result<FibSidecarIndex<u32>> = deserialize_index(&[]);
        assert!(result.is_err(), "empty file should be rejected");

        Ok(())
    }

    #[test]
    fn empty_index_roundtrips() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("empty.fbs1");

        let scorer = build_test_scorer()?;
        let index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);
        save_to_file(&index, &path)?;

        let loaded: FibSidecarIndex<u32> = load_from_file(&path)?;
        assert!(loaded.is_empty());
        assert_eq!(loaded.len(), 0);

        let d = loaded.scorer().quantizer().profile().ambient_dim as usize;
        let query = vec![0.0f32; d];
        let results = loaded.search(&query, 5, 1)?;
        assert!(results.is_empty());
        Ok(())
    }

    #[test]
    fn string_id_roundtrips() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("string_ids.fbs1");

        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<String> = FibSidecarIndex::new(scorer);
        let vectors = make_vectors(d, 8);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(format!("vec_{}", i), code);
        }
        save_to_file(&index, &path)?;

        let loaded: FibSidecarIndex<String> = load_from_file(&path)?;
        assert_eq!(loaded.len(), 8);

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = loaded.search(&query, 4, 1)?;
        assert_eq!(results.len(), 4);
        for r in &results {
            assert!(r.id.starts_with("vec_"), "id should be string vec_N");
        }
        Ok(())
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_search_matches_loaded_search() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("mmap_test.fbs1");

        let original = build_test_index()?;
        save_to_file(&original, &path)?;

        let loaded: FibSidecarIndex<u32> = load_from_file(&path)?;
        let mmap_index: MmapSidecarIndex<u32> = load_mmap(&path)?;

        assert_eq!(mmap_index.len(), loaded.len());

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let loaded_results = loaded.search(&query, 5, 2)?;
        let mmap_results = mmap_index.search(&query, 5, 2)?;

        assert_eq!(loaded_results.len(), mmap_results.len());
        for (a, b) in loaded_results.iter().zip(mmap_results.iter()) {
            assert_eq!(a.id, b.id, "id mismatch at rank {}", a.rank);
            assert_eq!(a.rank, b.rank);
            assert!(
                (a.approximate_score - b.approximate_score).abs() < 1e-5,
                "score mismatch at rank {}: {} vs {}",
                a.rank,
                a.approximate_score,
                b.approximate_score
            );
        }
        Ok(())
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn mmap_empty_index_works() -> Result<()> {
        let dir = tempdir()
            .map_err(|e| FibQuantError::CorruptPayload(format!("tempdir failed: {}", e)))?;
        let path = dir.path().join("mmap_empty.fbs1");

        let scorer = build_test_scorer()?;
        let index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);
        save_to_file(&index, &path)?;

        let mmap_index: MmapSidecarIndex<u32> = load_mmap(&path)?;
        assert!(mmap_index.is_empty());

        let d = mmap_index.profile().ambient_dim as usize;
        let query = vec![0.0f32; d];
        let results = mmap_index.search(&query, 5, 1)?;
        assert!(results.is_empty());
        Ok(())
    }
}
