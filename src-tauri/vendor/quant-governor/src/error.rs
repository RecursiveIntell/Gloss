//! Error types for quant-governor.

use thiserror::Error;

/// Errors that can occur during governance evaluation.
#[derive(Debug, Error)]
pub enum GovernorError {
    /// Invalid request parameters
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Policy evaluation failed
    #[error("Policy evaluation failed: {0}")]
    EvaluationFailed(String),

    /// Invalid degradation threshold
    #[error("Invalid degradation threshold: {0}")]
    InvalidThreshold(String),

    /// Content type not supported
    #[error("Unsupported content type: {0}")]
    UnsupportedContentType(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl GovernorError {
    /// Returns true if this is a recoverable error.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            GovernorError::InvalidThreshold(_) | GovernorError::UnsupportedContentType(_)
        )
    }

    /// Returns true if this indicates a system configuration issue.
    pub fn is_configuration_error(&self) -> bool {
        matches!(
            self,
            GovernorError::InvalidThreshold(_) | GovernorError::Internal(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_recoverability() {
        let err = GovernorError::InvalidThreshold("too high".to_string());
        assert!(err.is_recoverable());

        let err = GovernorError::Internal("bug".to_string());
        assert!(!err.is_recoverable());
    }

    #[test]
    fn error_display() {
        let err = GovernorError::InvalidRequest("bad value".to_string());
        assert_eq!(err.to_string(), "Invalid request: bad value");
    }
}
