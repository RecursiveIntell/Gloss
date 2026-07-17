//! High-throughput batch ingest pipeline for encoding and inserting
//! large vector corpora (100K+) into a [`FibSidecarIndex`].
//!
//! The pipeline wraps a [`FibQuantizer`] and [`FibSidecarIndex`], encoding
//! vectors in batches via [`FibQuantizer::encode_batch`] (which uses Rayon
//! parallelism when the `parallel` feature is enabled) and inserting the
//! resulting [`FibCodeV1`] artifacts into the sidecar index. Each batch
//! produces an [`IngestReceipt`] with timing, byte counts, and any errors.
//!
//! ## Throughput
//!
//! The pipeline is designed for 100K+ vector corpora. The encoding step
//! is dominated by [`FibQuantizer::encode_batch`], which dispatches the
//! per-vector codebook lookup across Rayon worker threads when the batch
//! is large enough (≥ 16 vectors). The index insertion step
//! ([`FibSidecarIndex::add_batch`]) is a simple `Vec::push` loop with no
//! per-entry allocation beyond the `(Id, FibCodeV1)` tuple.
//!
//! ## Example
//!
//! # use fib_quant::{BatchIngestPipeline, FibQuantProfileV1, FibQuantizer};
//! # fn main() -> fib_quant::Result<()> {
//! # let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
//! # profile.training_samples = 128;
//! # profile.lloyd_restarts = 1;
//! # profile.lloyd_iterations = 2;
//! # let quantizer = FibQuantizer::new(profile)?;
//! # let mut pipeline = BatchIngestPipeline::new(quantizer, 32)?;
//! # let items: Vec<(u32, Vec<f32>)> = (0..100)
//! #     .map(|i| (i as u32, vec![0.1 * i as f32 + 0.01; 8]))
//! #     .collect();
//! # let receipts = pipeline.ingest_from_iter(items.into_iter())?;
//! # let total: usize = receipts.iter().map(|r| r.batch_count).sum();
//! # assert_eq!(total, 100);
//! # let index = pipeline.finish();
//! # assert_eq!(index.len(), 100);
//! # Ok(())
//! # }
//! ```

use std::time::Instant;

use crate::{
    codec::FibCodeV1, scoring::FibScorer, sidecar::FibSidecarIndex, FibQuantError, FibQuantizer,
    Result,
};

/// Receipt documenting the outcome of a single batch ingest operation.
///
/// Produced by [`BatchIngestPipeline::ingest_batch`] for each batch of
/// vectors processed. The receipt captures the count of successfully
/// encoded/inserted vectors, the total encoded byte count (using
/// [`FibCodeV1::compact_size`]), the elapsed time, and any failures.
///
/// For multi-batch ingestion via
/// [`ingest_from_iter`](BatchIngestPipeline::ingest_from_iter), one
/// `IngestReceipt` is returned per chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestReceipt {
    /// Number of vectors successfully encoded and inserted in this batch.
    pub batch_count: usize,
    /// Total encoded bytes (sum of [`FibCodeV1::compact_size`] for all
    /// successfully encoded codes in this batch).
    pub total_bytes: u64,
    /// Elapsed time for this batch in microseconds (encode + insert).
    pub elapsed_micros: u128,
    /// Number of vectors that failed to encode or insert.
    pub failed: usize,
    /// Error messages for any failures (empty if all succeeded).
    pub error_messages: Vec<String>,
}

/// High-throughput batch ingest pipeline for encoding and inserting
/// large vector corpora into a [`FibSidecarIndex`].
///
/// The pipeline owns a [`FibQuantizer`] for encoding and a
/// [`FibSidecarIndex`] for storage. Vectors are processed in configurable
/// batch sizes via [`ingest_batch`](Self::ingest_batch) or
/// [`ingest_from_iter`](Self::ingest_from_iter), with each batch producing
/// an [`IngestReceipt`] for progress tracking.
///
/// ## Parallelism
///
/// [`FibQuantizer::encode_batch`] uses Rayon parallelism internally when
/// the `parallel` feature is enabled (default) and the batch is large
/// enough to amortize dispatch overhead (≥ 16 vectors). The pipeline
/// itself does not add additional parallelism — it relies on the codec's
/// internal parallelism for the compute-heavy encoding step.
///
/// ## Generics
///
/// `Id` must be `Clone + Eq + Debug`, matching the bound on
/// [`FibSidecarIndex`]. Common choices are `u64`, `String`, or a newtype
/// key.
pub struct BatchIngestPipeline<Id>
where
    Id: Clone + Eq + std::fmt::Debug,
{
    quantizer: FibQuantizer,
    index: FibSidecarIndex<Id>,
    batch_size: usize,
    total_encoded: usize,
    total_bytes: u64,
    failed: usize,
}

impl<Id> BatchIngestPipeline<Id>
where
    Id: Clone + Eq + std::fmt::Debug,
{
    /// Create a new batch ingest pipeline.
    ///
    /// Builds a [`FibScorer`] from the quantizer (cloning it so the
    /// pipeline retains its own copy for encoding) and creates an empty
    /// [`FibSidecarIndex`].
    ///
    /// `batch_size` is the default chunk size used by
    /// [`ingest_from_iter`](Self::ingest_from_iter). It must be > 0.
    ///
    /// # Errors
    ///
    /// Returns [`FibQuantError::CorruptPayload`] if `batch_size` is 0.
    pub fn new(quantizer: FibQuantizer, batch_size: usize) -> Result<Self> {
        if batch_size == 0 {
            return Err(FibQuantError::CorruptPayload(
                "batch_size must be > 0".into(),
            ));
        }
        let scorer = FibScorer::new(quantizer.clone())?;
        let index = FibSidecarIndex::new(scorer);
        Ok(Self {
            quantizer,
            index,
            batch_size,
            total_encoded: 0,
            total_bytes: 0,
            failed: 0,
        })
    }

    /// Ingest a batch of `(Id, vector)` pairs.
    ///
    /// Extracts all f32 vector slices, calls [`FibQuantizer::encode_batch`]
    /// for parallel encoding (Rayon when the `parallel` feature is enabled
    /// and the batch is ≥ 16 vectors), then adds all encoded codes to the
    /// sidecar index via [`FibSidecarIndex::add_batch`]. Returns an
    /// [`IngestReceipt`] with the count, bytes, timing, and any errors.
    ///
    /// If encoding fails for the entire batch (e.g., dimension mismatch or
    /// a zero-norm vector), the receipt will have `failed = items.len()`
    /// and the error will be recorded in `error_messages`. The index is
    /// not modified in this case.
    ///
    /// # Empty batches
    ///
    /// An empty `items` slice returns a zero-count receipt immediately
    /// without touching the quantizer or index.
    pub fn ingest_batch(&mut self, items: &[(Id, &[f32])]) -> Result<IngestReceipt> {
        let started = Instant::now();

        if items.is_empty() {
            return Ok(IngestReceipt {
                batch_count: 0,
                total_bytes: 0,
                elapsed_micros: 0,
                failed: 0,
                error_messages: Vec::new(),
            });
        }

        // Extract vector slices for encode_batch
        let vectors: Vec<&[f32]> = items.iter().map(|(_, v)| *v).collect();

        // Encode all vectors (uses Rayon parallelism internally)
        let codes = match self.quantizer.encode_batch(&vectors) {
            Ok(codes) => codes,
            Err(e) => {
                let msg = format!("encode_batch failed: {e}");
                self.failed += items.len();
                return Ok(IngestReceipt {
                    batch_count: 0,
                    total_bytes: 0,
                    elapsed_micros: started.elapsed().as_micros(),
                    failed: items.len(),
                    error_messages: vec![msg],
                });
            }
        };

        // Pair IDs with codes and add to index
        let entries: Vec<(Id, FibCodeV1)> =
            items.iter().map(|(id, _)| id.clone()).zip(codes).collect();

        // Track bytes before moving entries into the index
        let bytes: u64 = entries
            .iter()
            .map(|(_, code)| code.compact_size() as u64)
            .sum();
        let count = entries.len();

        self.index.add_batch(entries);
        self.total_encoded += count;
        self.total_bytes += bytes;

        let elapsed = started.elapsed().as_micros();

        Ok(IngestReceipt {
            batch_count: count,
            total_bytes: bytes,
            elapsed_micros: elapsed,
            failed: 0,
            error_messages: Vec::new(),
        })
    }

    /// Ingest vectors from an iterator, chunking into `batch_size` pieces.
    ///
    /// Each chunk is passed to [`ingest_batch`](Self::ingest_batch), and
    /// all receipts are collected and returned. This is the primary entry
    /// point for streaming large corpora (100K+ vectors) through the
    /// pipeline.
    ///
    /// The iterator is consumed lazily — only one chunk is held in memory
    /// at a time (plus the receipts vector). This keeps memory usage
    /// bounded regardless of the total corpus size.
    pub fn ingest_from_iter<I>(&mut self, iter: I) -> Result<Vec<IngestReceipt>>
    where
        I: Iterator<Item = (Id, Vec<f32>)>,
    {
        let mut receipts = Vec::new();
        let mut chunk: Vec<(Id, Vec<f32>)> = Vec::with_capacity(self.batch_size);

        for item in iter {
            chunk.push(item);
            if chunk.len() >= self.batch_size {
                // Convert owned Vecs to slices for ingest_batch
                let refs: Vec<(Id, &[f32])> = chunk
                    .iter()
                    .map(|(id, v)| (id.clone(), v.as_slice()))
                    .collect();
                receipts.push(self.ingest_batch(&refs)?);
                chunk.clear();
            }
        }

        // Process remaining items in the final partial chunk
        if !chunk.is_empty() {
            let refs: Vec<(Id, &[f32])> = chunk
                .iter()
                .map(|(id, v)| (id.clone(), v.as_slice()))
                .collect();
            receipts.push(self.ingest_batch(&refs)?);
        }

        Ok(receipts)
    }

    /// Consume the pipeline and return the completed [`FibSidecarIndex`].
    ///
    /// The index contains all successfully encoded and inserted entries.
    /// After calling `finish`, the pipeline is consumed and can no longer
    /// be used.
    pub fn finish(self) -> FibSidecarIndex<Id> {
        self.index
    }

    /// Number of vectors successfully encoded and inserted so far.
    pub fn total_encoded(&self) -> usize {
        self.total_encoded
    }

    /// Total encoded bytes accumulated so far.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Number of vectors that failed to encode or insert so far.
    pub fn failed(&self) -> usize {
        self.failed
    }

    /// Configured batch size.
    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Current number of entries in the underlying index.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FibQuantProfileV1;

    fn build_test_quantizer() -> FibQuantizer {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7).unwrap();
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        FibQuantizer::new(profile).unwrap()
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

    #[test]
    fn ingest_100_vectors_in_batches_of_32() -> Result<()> {
        let quantizer = build_test_quantizer();
        let d = quantizer.profile().ambient_dim as usize;
        let mut pipeline = BatchIngestPipeline::new(quantizer, 32)?;

        let vectors = make_vectors(d, 100);
        let items: Vec<(u32, Vec<f32>)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.clone()))
            .collect();

        let receipts = pipeline.ingest_from_iter(items.into_iter())?;

        // 100 vectors / 32 per batch = 4 batches (32, 32, 32, 4)
        assert_eq!(receipts.len(), 4, "should produce 4 batch receipts");
        assert_eq!(receipts[0].batch_count, 32);
        assert_eq!(receipts[1].batch_count, 32);
        assert_eq!(receipts[2].batch_count, 32);
        assert_eq!(receipts[3].batch_count, 4, "last batch should have 4 items");

        let total_count: usize = receipts.iter().map(|r| r.batch_count).sum();
        assert_eq!(total_count, 100);

        let index = pipeline.finish();
        assert_eq!(index.len(), 100, "index should have 100 entries");
        assert!(!index.is_empty());

        Ok(())
    }

    #[test]
    fn search_works_after_ingest() -> Result<()> {
        let quantizer = build_test_quantizer();
        let d = quantizer.profile().ambient_dim as usize;
        let mut pipeline = BatchIngestPipeline::new(quantizer, 32)?;

        let vectors = make_vectors(d, 100);
        let items: Vec<(u32, Vec<f32>)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.clone()))
            .collect();

        pipeline.ingest_from_iter(items.into_iter())?;

        let index = pipeline.finish();
        assert_eq!(index.len(), 100);

        // Verify search returns the correct number of candidates
        let query = &vectors[0];
        let results = index.search(query, 5, 1)?;
        assert_eq!(results.len(), 5, "search should return top_k=5 candidates");

        // Ranks should be sequential from 0
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.rank, i, "rank should be sequential from 0");
        }

        // Results should be sorted by descending approximate score
        for w in results.windows(2) {
            assert!(
                w[0].approximate_score >= w[1].approximate_score,
                "results should be sorted descending by score"
            );
        }

        // Verify search with receipt also works
        let (results2, receipt) = index.search_with_receipt(query, 3, 2)?;
        assert_eq!(results2.len(), 6, "top_k=3 oversample=2 should give 6");
        assert_eq!(receipt.indexed_count, 100);
        assert_eq!(receipt.top_k, 3);
        assert_eq!(receipt.oversample, 2);

        Ok(())
    }

    #[test]
    fn receipt_fields_are_correct() -> Result<()> {
        let quantizer = build_test_quantizer();
        let d = quantizer.profile().ambient_dim as usize;
        let mut pipeline = BatchIngestPipeline::new(quantizer, 1024)?;

        let vectors = make_vectors(d, 50);
        let refs: Vec<(u32, &[f32])> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.as_slice()))
            .collect();

        let receipt = pipeline.ingest_batch(&refs)?;

        assert_eq!(
            receipt.batch_count, 50,
            "batch_count should match input size"
        );
        assert_eq!(receipt.failed, 0, "no failures expected for valid vectors");
        assert!(
            receipt.total_bytes > 0,
            "total_bytes should be positive for 50 encoded codes"
        );
        assert!(
            receipt.elapsed_micros > 0 || receipt.batch_count == 0,
            "elapsed_micros should be positive for non-empty batch"
        );
        assert!(
            receipt.error_messages.is_empty(),
            "no error messages expected for valid input"
        );

        Ok(())
    }

    #[test]
    fn empty_batch_returns_zero_count_receipt() -> Result<()> {
        let quantizer = build_test_quantizer();
        let mut pipeline = BatchIngestPipeline::new(quantizer, 1024)?;

        let empty: Vec<(u32, &[f32])> = vec![];
        let receipt = pipeline.ingest_batch(&empty)?;

        assert_eq!(receipt.batch_count, 0);
        assert_eq!(receipt.total_bytes, 0);
        assert_eq!(receipt.failed, 0);
        assert!(receipt.error_messages.is_empty());

        Ok(())
    }

    #[test]
    fn finish_returns_index_with_all_entries() -> Result<()> {
        let quantizer = build_test_quantizer();
        let d = quantizer.profile().ambient_dim as usize;
        let mut pipeline = BatchIngestPipeline::new(quantizer, 16)?;

        let vectors = make_vectors(d, 40);
        let items: Vec<(u32, Vec<f32>)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u32, v.clone()))
            .collect();

        pipeline.ingest_from_iter(items.into_iter())?;

        assert_eq!(pipeline.total_encoded(), 40);
        assert_eq!(pipeline.failed(), 0);
        assert!(pipeline.total_bytes() > 0);
        assert_eq!(pipeline.len(), 40);

        let index = pipeline.finish();
        assert_eq!(index.len(), 40);

        Ok(())
    }
}
