/// Error types for the semantic-memory crate.
///
/// All errors flow through [`MemoryError`], using `#[from]` for automatic
/// conversion from rusqlite and reqwest errors.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// SQLite / rusqlite error.
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// HTTP error from the embedding provider.
    #[error("Embedding request failed: {0}")]
    EmbeddingRequest(#[from] reqwest::Error),

    /// Embedding vector has wrong number of dimensions.
    #[error("Embedding provider returned {actual} dimensions, expected {expected}")]
    DimensionMismatch { expected: usize, actual: usize },

    /// Embedding provider returned a different number of vectors than requested.
    #[error("Embedding provider returned {returned} vectors, expected {requested}")]
    EmbeddingBatchCountMismatch { requested: usize, returned: usize },

    /// Embedding vector has wrong number of dimensions.
    #[error("Embedding vector has {actual} dimensions, expected {expected}")]
    EmbeddingDimensionMismatch { expected: usize, actual: usize },

    /// Embedding vector contains NaN or infinity.
    #[error("Embedding vector contains non-finite value at index {index}")]
    NonFiniteEmbeddingValue { index: usize },

    /// Raw vector BLOB length does not match the expected f32 dimensions.
    #[error("Vector blob length mismatch: expected {expected_bytes} bytes, got {actual_bytes}")]
    VectorBlobLengthMismatch {
        expected_bytes: usize,
        actual_bytes: usize,
    },

    /// Encoded vector artifact was produced with a different codec profile.
    #[error("Vector codec profile mismatch: expected {expected_digest}, got {actual_digest}")]
    VectorCodecProfileMismatch {
        /// Digest required by the decoding codec.
        expected_digest: String,
        /// Digest carried by the encoded artifact.
        actual_digest: String,
    },

    /// A durable search receipt ID already exists with different payload bytes.
    #[error("Search receipt ID conflict for {receipt_id}")]
    SearchReceiptConflict {
        /// Conflicting receipt/request ID.
        receipt_id: String,
    },

    /// Canonical content digest computation failed.
    #[error("Digest error: {0}")]
    DigestError(String),

    /// A requested durable search receipt was not found.
    #[error("Search receipt not found: {receipt_id}")]
    SearchReceiptNotFound {
        /// Requested receipt/request ID.
        receipt_id: String,
    },

    /// Raw BLOB data is not a valid embedding.
    #[error("Invalid embedding data: expected {expected_bytes} bytes, got {actual_bytes}")]
    InvalidEmbedding {
        expected_bytes: usize,
        actual_bytes: usize,
    },

    /// Database was created with a different embedding model.
    #[error("Embedding model mismatch: database has '{stored}', config specifies '{configured}'")]
    ModelMismatch { stored: String, configured: String },

    /// Session with the given ID does not exist.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Fact with the given ID does not exist.
    #[error("Fact not found: {0}")]
    FactNotFound(String),

    /// Document with the given ID does not exist.
    #[error("Document not found: {0}")]
    DocumentNotFound(String),

    /// Embedding provider is unreachable or misconfigured.
    #[error("Embedding provider unavailable: {0}")]
    EmbedderUnavailable(String),

    /// Database migration failed.
    #[error("Migration failed at version {version}: {reason}")]
    MigrationFailed { version: u32, reason: String },

    /// HNSW index error.
    #[error("HNSW index error: {0}")]
    HnswError(String),

    /// Invalid HNSW key format.
    #[error("Invalid HNSW key format: {0}")]
    InvalidKey(String),

    /// Quantization error.
    #[error("Quantization error: {0}")]
    QuantizationError(String),

    /// Storage path error.
    #[error("Storage path error: {0}")]
    StorageError(String),

    /// Index integrity check failed.
    #[error("Index integrity check failed: {in_sqlite_not_hnsw} items in SQLite but not HNSW, {in_hnsw_not_sqlite} items in HNSW but not SQLite")]
    IntegrityError {
        in_sqlite_not_hnsw: usize,
        in_hnsw_not_sqlite: usize,
    },

    /// Database schema is newer than this library version can handle.
    #[error(
        "Schema version {found} is ahead of max supported {supported} — upgrade semantic-memory"
    )]
    SchemaAhead {
        /// Schema version found in the database.
        found: u32,
        /// Maximum version supported by this build.
        supported: u32,
    },

    /// Content exceeds configured size limit.
    #[error("Content too large: {size} bytes exceeds limit of {limit} bytes")]
    ContentTooLarge {
        /// Actual content size in bytes.
        size: usize,
        /// Configured limit in bytes.
        limit: usize,
    },

    /// Namespace fact count would exceed the configured limit.
    #[error("Namespace '{namespace}' has {count} facts, limit is {limit}")]
    NamespaceFull {
        /// Namespace that is full.
        namespace: String,
        /// Current fact count.
        count: usize,
        /// Configured limit.
        limit: usize,
    },

    /// The configured database size ceiling would be exceeded by a new write.
    #[error("Database size limit exceeded: current footprint is {current} bytes, limit is {limit} bytes")]
    DatabaseSizeLimitExceeded {
        /// Current observed database footprint in bytes.
        current: u64,
        /// Configured limit in bytes.
        limit: u64,
    },

    /// Episode with the given ID does not exist.
    #[error("Episode not found: {0}")]
    EpisodeNotFound(String),

    /// Connection pool reader acquisition timed out.
    #[error("Pool reader acquisition timed out after {elapsed_ms}ms (pool size: {pool_size})")]
    PoolTimeout {
        /// How long the caller waited before giving up.
        elapsed_ms: u64,
        /// Number of reader slots in the pool.
        pool_size: usize,
    },

    /// Brute-force vector search would scan more rows than the configured hard limit.
    #[error(
        "Vector scan hard limit exceeded for {table}: scanned {scanned} rows, limit is {limit}"
    )]
    VectorScanLimitExceeded {
        /// Logical table/collection being scanned.
        table: String,
        /// Rows scanned before the circuit breaker tripped.
        scanned: usize,
        /// Configured hard limit.
        limit: usize,
    },

    /// Configuration could not be normalized into a valid runtime state.
    #[error("Invalid configuration for '{field}': {reason}")]
    InvalidConfig {
        /// The config field or section that failed validation.
        field: &'static str,
        /// Human-readable explanation of the invalid value.
        reason: String,
    },

    /// Stored data is malformed or internally inconsistent.
    #[error("Corrupt data in {table} ({row_id}): {detail}")]
    CorruptData {
        /// Table or logical collection containing the bad row.
        table: &'static str,
        /// Primary key / row identifier for the corrupt record.
        row_id: String,
        /// Human-readable description of the corruption.
        detail: String,
    },

    /// Import envelope is structurally invalid.
    #[error("Invalid import envelope: {reason}")]
    ImportInvalid {
        /// What is wrong with the envelope.
        reason: String,
    },

    /// Import envelope has already been ingested (idempotent duplicate).
    #[error("Import envelope already ingested: {envelope_id}")]
    ImportDuplicate {
        /// The duplicate envelope ID.
        envelope_id: String,
    },

    /// Import hit a historical digest/receipt drift seam and needs operator repair.
    #[error(
        "Import requires digest migration or receipt repair for {source_envelope_id}: {detail}"
    )]
    ImportMigrationRequired {
        /// The source envelope whose historical import receipts no longer line up.
        source_envelope_id: String,
        /// Human-readable conflict details and operator guidance.
        detail: String,
    },

    /// Catch-all for other errors.
    #[error("{0}")]
    Other(String),
}

impl MemoryError {
    /// Returns a stable string discriminant for programmatic matching.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::EmbeddingRequest(_) => "embedding_request",
            Self::DimensionMismatch { .. } => "dimension_mismatch",
            Self::EmbeddingBatchCountMismatch { .. } => "embedding_batch_count_mismatch",
            Self::EmbeddingDimensionMismatch { .. } => "embedding_dimension_mismatch",
            Self::NonFiniteEmbeddingValue { .. } => "non_finite_embedding_value",
            Self::VectorBlobLengthMismatch { .. } => "vector_blob_length_mismatch",
            Self::VectorCodecProfileMismatch { .. } => "vector_codec_profile_mismatch",
            Self::SearchReceiptConflict { .. } => "search_receipt_conflict",
            Self::DigestError(_) => "digest_error",
            Self::SearchReceiptNotFound { .. } => "search_receipt_not_found",
            Self::InvalidEmbedding { .. } => "invalid_embedding",
            Self::ModelMismatch { .. } => "model_mismatch",
            Self::SessionNotFound(_) => "session_not_found",
            Self::FactNotFound(_) => "fact_not_found",
            Self::DocumentNotFound(_) => "document_not_found",
            Self::EpisodeNotFound(_) => "episode_not_found",
            Self::PoolTimeout { .. } => "pool_timeout",
            Self::VectorScanLimitExceeded { .. } => "vector_scan_limit_exceeded",
            Self::EmbedderUnavailable(_) => "embedder_unavailable",
            Self::MigrationFailed { .. } => "migration_failed",
            Self::HnswError(_) => "hnsw_error",
            Self::InvalidKey(_) => "invalid_key",
            Self::QuantizationError(_) => "quantization_error",
            Self::StorageError(_) => "storage_error",
            Self::IntegrityError { .. } => "integrity_error",
            Self::SchemaAhead { .. } => "schema_ahead",
            Self::ContentTooLarge { .. } => "content_too_large",
            Self::NamespaceFull { .. } => "namespace_full",
            Self::DatabaseSizeLimitExceeded { .. } => "database_size_limit_exceeded",
            Self::InvalidConfig { .. } => "invalid_config",
            Self::CorruptData { .. } => "corrupt_data",
            Self::ImportInvalid { .. } => "import_invalid",
            Self::ImportDuplicate { .. } => "import_duplicate",
            Self::ImportMigrationRequired { .. } => "import_migration_required",
            Self::Other(_) => "other",
        }
    }
}
