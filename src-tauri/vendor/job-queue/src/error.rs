use thiserror::Error;

/// Errors that can occur in the queue system.
#[derive(Error, Debug)]
pub enum QueueError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Job execution failed: {0}")]
    Execution(String),

    #[error("Job not found: {0}")]
    NotFound(String),

    #[error("Invalid state transition for job '{job_id}': {from} → {to}")]
    InvalidTransition {
        job_id: String,
        from: String,
        to: String,
    },

    #[error("Queue is paused")]
    Paused,

    #[error("Job was cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl QueueError {
    /// Stable string discriminant for structured logging (PRIMITIVES_CONTRACT §2).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::Serialization(_) => "serialization",
            Self::Execution(_) => "execution",
            Self::NotFound(_) => "not_found",
            Self::InvalidTransition { .. } => "invalid_transition",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::Other(_) => "other",
        }
    }
}

impl From<anyhow::Error> for QueueError {
    fn from(err: anyhow::Error) -> Self {
        QueueError::Other(err.to_string())
    }
}

impl<T> From<std::sync::PoisonError<T>> for QueueError {
    fn from(err: std::sync::PoisonError<T>) -> Self {
        QueueError::Other(err.to_string())
    }
}
