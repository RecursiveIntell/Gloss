use serde::Serialize;

use crate::{FibQuantError, Result};

/// Compute a deterministic BLAKE3 digest over a domain tag and JSON payload.
pub fn json_digest<T: Serialize>(domain: &str, value: &T) -> Result<String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    serde_json::to_writer(&mut bytes, value)
        .map_err(|err| FibQuantError::NumericalFailure(format!("json digest encode: {err}")))?;
    Ok(bytes_digest(&bytes))
}

/// Compute a BLAKE3 digest over raw bytes.
pub fn bytes_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}
