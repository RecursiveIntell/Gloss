use crate::{FibQuantError, Result};

/// Pack fixed-width indices into little-endian bit order.
pub fn pack_indices(indices: &[u32], width: u8) -> Result<Vec<u8>> {
    if width == 0 || width > 32 {
        return Err(FibQuantError::CorruptPayload(format!(
            "invalid bit width {width}"
        )));
    }
    let total_bits = indices
        .len()
        .checked_mul(width as usize)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("index bit count overflow".into()))?;
    let expected_bytes = total_bits
        .checked_add(7)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("index byte count overflow".into()))?
        / 8;
    let mut out = vec![0u8; expected_bytes];
    let max = if width == 32 {
        u32::MAX
    } else {
        (1u32 << width) - 1
    };
    for (idx, &value) in indices.iter().enumerate() {
        if value > max {
            return Err(FibQuantError::IndexOutOfRange {
                index: value,
                codebook_size: max,
            });
        }
        let start = idx.checked_mul(width as usize).ok_or_else(|| {
            FibQuantError::ResourceLimitExceeded("index bit offset overflow".into())
        })?;
        for bit in 0..width as usize {
            if ((value >> bit) & 1) == 1 {
                let pos = start.checked_add(bit).ok_or_else(|| {
                    FibQuantError::ResourceLimitExceeded("index bit position overflow".into())
                })?;
                out[pos / 8] |= 1 << (pos % 8);
            }
        }
    }
    Ok(out)
}

/// Unpack fixed-width indices and reject nonzero padding bits.
pub fn unpack_indices(bytes: &[u8], count: usize, width: u8) -> Result<Vec<u32>> {
    if width == 0 || width > 32 {
        return Err(FibQuantError::CorruptPayload(format!(
            "invalid bit width {width}"
        )));
    }
    let total_bits = count
        .checked_mul(width as usize)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("index bit count overflow".into()))?;
    let expected_bytes = total_bits
        .checked_add(7)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("index byte count overflow".into()))?
        / 8;
    if bytes.len() != expected_bytes {
        return Err(FibQuantError::CorruptPayload(format!(
            "payload has {} bytes, expected {expected_bytes}",
            bytes.len()
        )));
    }
    for pos in total_bits..(expected_bytes * 8) {
        if ((bytes[pos / 8] >> (pos % 8)) & 1) == 1 {
            return Err(FibQuantError::CorruptPayload(
                "nonzero fixed-rate padding bits".into(),
            ));
        }
    }
    let mut indices = Vec::with_capacity(count);
    for idx in 0..count {
        let start = idx.checked_mul(width as usize).ok_or_else(|| {
            FibQuantError::ResourceLimitExceeded("index bit offset overflow".into())
        })?;
        let mut value = 0u32;
        for bit in 0..width as usize {
            let pos = start.checked_add(bit).ok_or_else(|| {
                FibQuantError::ResourceLimitExceeded("index bit position overflow".into())
            })?;
            if ((bytes[pos / 8] >> (pos % 8)) & 1) == 1 {
                value |= 1 << bit;
            }
        }
        indices.push(value);
    }
    Ok(indices)
}
