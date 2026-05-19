//! Bitpacking helpers for production codec payloads.
//!
//! Packing order is little-endian within each byte: the first logical value
//! starts at bit 0 of byte 0.

use crate::error::{Result, TurboQuantError};

/// Return the number of bytes needed to store `count` values at `bits` bits each.
pub fn packed_len(count: usize, bits: u8) -> Result<usize> {
    validate_bits(bits)?;
    let total_bits =
        count
            .checked_mul(bits as usize)
            .ok_or_else(|| TurboQuantError::MalformedCode {
                reason: "packed bit length overflow".into(),
            })?;
    Ok(total_bits.div_ceil(8))
}

/// Pack integer indices, each of which must fit in `bits` bits.
pub fn pack_indices(indices: &[u16], bits: u8) -> Result<Vec<u8>> {
    validate_bits(bits)?;
    let levels = levels(bits);
    let mut packed = vec![0u8; packed_len(indices.len(), bits)?];
    for (index, &value) in indices.iter().enumerate() {
        if u32::from(value) >= levels {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("index {index} value {value} is outside [0, {levels})"),
            });
        }
        write_bits(&mut packed, index * bits as usize, bits, u32::from(value));
    }
    Ok(packed)
}

/// Unpack integer indices from packed bytes.
pub fn unpack_indices(packed: &[u8], count: usize, bits: u8) -> Result<Vec<u16>> {
    validate_packed_len(packed, count, bits)?;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        out.push(read_bits(packed, index * bits as usize, bits) as u16);
    }
    validate_padding_zero(packed, count * bits as usize)?;
    Ok(out)
}

/// Pack QJL signs. The convention is `0 => -1`, `1 => +1`.
pub fn pack_signs(signs: &[i8]) -> Result<Vec<u8>> {
    let mut packed = vec![0u8; signs.len().div_ceil(8)];
    for (index, &sign) in signs.iter().enumerate() {
        match sign {
            -1 => {}
            1 => packed[index / 8] |= 1 << (index % 8),
            other => {
                return Err(TurboQuantError::MalformedCode {
                    reason: format!("sign {index} is {other}, expected -1 or 1"),
                });
            }
        }
    }
    Ok(packed)
}

/// Unpack QJL signs. The convention is `0 => -1`, `1 => +1`.
pub fn unpack_signs(packed: &[u8], count: usize) -> Result<Vec<i8>> {
    let expected = count.div_ceil(8);
    if packed.len() != expected {
        return Err(TurboQuantError::MalformedCode {
            reason: format!(
                "packed sign payload has {} bytes, expected {expected}",
                packed.len()
            ),
        });
    }
    validate_padding_zero(packed, count)?;
    Ok((0..count)
        .map(|index| {
            if (packed[index / 8] >> (index % 8)) & 1 == 1 {
                1
            } else {
                -1
            }
        })
        .collect())
}

/// Validate that all bits after `used_bits` are zero.
pub fn validate_padding_zero(packed: &[u8], used_bits: usize) -> Result<()> {
    let used_bytes = used_bits.div_ceil(8);
    if packed.len() != used_bytes {
        return Err(TurboQuantError::MalformedCode {
            reason: format!(
                "packed payload has {} bytes, expected {used_bytes}",
                packed.len()
            ),
        });
    }
    let used_bits_in_final_byte = used_bits % 8;
    if used_bits_in_final_byte == 0 || packed.is_empty() {
        return Ok(());
    }
    let unused_mask = !((1u8 << used_bits_in_final_byte) - 1);
    if packed[packed.len() - 1] & unused_mask != 0 {
        return Err(TurboQuantError::MalformedCode {
            reason: "nonzero padding bits in packed payload".into(),
        });
    }
    Ok(())
}

pub(crate) fn validate_packed_len(packed: &[u8], count: usize, bits: u8) -> Result<()> {
    let expected = packed_len(count, bits)?;
    if packed.len() != expected {
        return Err(TurboQuantError::MalformedCode {
            reason: format!(
                "packed index payload has {} bytes, expected {expected}",
                packed.len()
            ),
        });
    }
    Ok(())
}

fn validate_bits(bits: u8) -> Result<()> {
    if bits == 0 || bits > 16 {
        return Err(TurboQuantError::InvalidBitWidth { got: bits });
    }
    Ok(())
}

fn levels(bits: u8) -> u32 {
    1u32 << bits
}

fn write_bits(bytes: &mut [u8], bit_offset: usize, bits: u8, mut value: u32) {
    for bit in 0..bits as usize {
        if value & 1 == 1 {
            let absolute = bit_offset + bit;
            bytes[absolute / 8] |= 1 << (absolute % 8);
        }
        value >>= 1;
    }
}

fn read_bits(bytes: &[u8], bit_offset: usize, bits: u8) -> u32 {
    let mut value = 0u32;
    for bit in 0..bits as usize {
        let absolute = bit_offset + bit;
        let source = (bytes[absolute / 8] >> (absolute % 8)) & 1;
        value |= u32::from(source) << bit;
    }
    value
}
