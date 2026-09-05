#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct EvalReport {
    pub mse: Option<f64>,
    pub cosine_similarity: Option<f64>,
    pub max_abs_error: Option<f64>,
    pub bytes_exact: u64,
    pub bytes_encoded: u64,
    pub passed: bool,
    pub notes: Vec<String>,
}

impl EvalReport {
    pub fn exact(bytes_exact: u64) -> Self {
        Self {
            mse: Some(0.0),
            cosine_similarity: Some(1.0),
            max_abs_error: Some(0.0),
            bytes_exact,
            bytes_encoded: bytes_exact,
            passed: true,
            notes: vec!["exact representation".to_string()],
        }
    }
}
