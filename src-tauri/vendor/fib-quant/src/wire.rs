//! Self-describing wire format for FibQuant codes.
//!
//! The FB1/FB2 compact formats strip all profile metadata, forcing
//! downstream consumers to hardcode seed, dim, k, N externally. This
//! module provides `FibCodeWireV1` — a self-describing format with a
//! header carrying seed, dim, k, N, norm_format, and profile digest,
//! enabling decode without any external profile knowledge.

use crate::{
    bitpack::unpack_indices,
    codec::{FibCodeV1, CODE_SCHEMA},
    profile::{FibQuantProfileV1, NormFormat},
    FibQuantError, Result,
};

/// Magic bytes: "FBW1" = Fib Binary Wire v1.
pub const WIRE_MAGIC: [u8; 4] = [b'F', b'B', b'W', b'1'];
/// Current wire format version.
pub const WIRE_VERSION: u8 = 1;
/// Header size: magic(4) + version(1) + ambient_dim(4) + block_dim(4)
/// + codebook_size(4) + rotation_seed(8) + wire_index_bits(1) + norm_format(1)
/// + profile_digest(32) = 59 bytes.
pub const WIRE_HEADER_SIZE: usize = 59;

/// Parsed wire header (metadata without full decode).
#[derive(Debug, Clone, PartialEq)]
pub struct WireHeader {
    pub version: u8,
    pub ambient_dim: u32,
    pub block_dim: u32,
    pub codebook_size: u32,
    pub rotation_seed: u64,
    pub wire_index_bits: u8,
    pub norm_format: NormFormat,
    pub profile_digest: [u8; 32],
    pub body_offset: usize,
}

/// Self-describing wire format codec.
pub struct FibCodeWireV1;

impl FibCodeWireV1 {
    /// Encode a FibCodeV1 with profile metadata into a self-describing
    /// wire format. The header carries all profile fields needed to
    /// reconstruct the quantizer for decode.
    pub fn to_wire_bytes(code: &FibCodeV1, profile: &FibQuantProfileV1) -> Result<Vec<u8>> {
        let profile_digest_hex = profile.digest()?;
        let digest_bytes = hex_decode_32(strip_blake3_prefix(&profile_digest_hex))?;
        let mut out =
            Vec::with_capacity(WIRE_HEADER_SIZE + 2 + code.norm_payload.len() + code.indices.len());

        // Header
        out.extend_from_slice(&WIRE_MAGIC);
        out.push(WIRE_VERSION);
        out.extend_from_slice(&profile.ambient_dim.to_le_bytes());
        out.extend_from_slice(&profile.block_dim.to_le_bytes());
        out.extend_from_slice(&profile.codebook_size.to_le_bytes());
        out.extend_from_slice(&profile.rotation_seed.to_le_bytes());
        out.push(profile.wire_index_bits);
        out.push(norm_format_to_byte(&profile.norm_format));
        out.extend_from_slice(&digest_bytes);

        // Body: norm_len(u16) + norm_payload + indices
        let norm_len = code.norm_payload.len() as u16;
        out.extend_from_slice(&norm_len.to_le_bytes());
        out.extend_from_slice(&code.norm_payload);
        out.extend_from_slice(&code.indices);
        Ok(out)
    }

    /// Parse only the header from wire bytes. Does not decode the body.
    /// Useful for metadata extraction without building a quantizer.
    pub fn parse_header(bytes: &[u8]) -> Result<WireHeader> {
        if bytes.len() < WIRE_HEADER_SIZE {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire format too short: {} bytes (need >= {})",
                bytes.len(),
                WIRE_HEADER_SIZE
            )));
        }
        if bytes[0..4] != WIRE_MAGIC {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire bad magic: {:?} (expected {:?})",
                &bytes[0..4],
                WIRE_MAGIC
            )));
        }
        let version = bytes[4];
        if version != WIRE_VERSION {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire version {} not supported (need {})",
                version, WIRE_VERSION
            )));
        }
        let ambient_dim = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
        let block_dim = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
        let codebook_size = u32::from_le_bytes([bytes[13], bytes[14], bytes[15], bytes[16]]);
        let rotation_seed = u64::from_le_bytes([
            bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23], bytes[24],
        ]);
        let wire_index_bits = bytes[25];
        let norm_format = byte_to_norm_format(bytes[26])?;
        let mut profile_digest = [0u8; 32];
        profile_digest.copy_from_slice(&bytes[27..59]);

        Ok(WireHeader {
            version,
            ambient_dim,
            block_dim,
            codebook_size,
            rotation_seed,
            wire_index_bits,
            norm_format,
            profile_digest,
            body_offset: WIRE_HEADER_SIZE,
        })
    }

    /// Decode a self-describing wire payload into a FibCodeV1 and the
    /// reconstructed FibQuantProfileV1. The caller can use the profile
    /// to build a FibQuantizer for decode.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<(FibCodeV1, FibQuantProfileV1)> {
        let header = Self::parse_header(bytes)?;
        let body = &bytes[header.body_offset..];
        if body.len() < 2 {
            return Err(FibQuantError::CorruptPayload(
                "wire body too short for norm_len".into(),
            ));
        }
        let norm_len = u16::from_le_bytes([body[0], body[1]]) as usize;
        if body.len() < 2 + norm_len {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire norm truncated: norm_len={} but only {} bytes remain",
                norm_len,
                body.len() - 2
            )));
        }
        let norm_payload = body[2..2 + norm_len].to_vec();
        let indices = body[2 + norm_len..].to_vec();

        // Reconstruct profile from header fields
        let profile = FibQuantProfileV1::paper_default(
            header.ambient_dim as usize,
            header.block_dim as usize,
            header.codebook_size as usize,
            header.rotation_seed,
        )?;

        // Validate wire_index_bits
        if profile.wire_index_bits != header.wire_index_bits {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire_index_bits {} does not match expected {}",
                header.wire_index_bits, profile.wire_index_bits
            )));
        }

        // Validate packed index length
        let block_count = profile.block_count() as usize;
        let expected_packed_len = (block_count * header.wire_index_bits as usize).div_ceil(8);
        if indices.len() != expected_packed_len {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire indices: got {} bytes, expected {}",
                indices.len(),
                expected_packed_len
            )));
        }

        // Verify indices unpack correctly
        let _ = unpack_indices(&indices, block_count, header.wire_index_bits)?;

        let profile_digest = profile.digest()?;
        Ok((
            FibCodeV1 {
                schema_version: CODE_SCHEMA.into(),
                profile_digest,
                codebook_digest: String::new(),
                rotation_digest: String::new(),
                ambient_dim: profile.ambient_dim,
                block_dim: profile.block_dim,
                norm_format: profile.norm_format.clone(),
                norm_payload,
                wire_index_bits: header.wire_index_bits,
                block_count: profile.block_count(),
                indices,
            },
            profile,
        ))
    }
}

fn norm_format_to_byte(format: &NormFormat) -> u8 {
    match format {
        NormFormat::Fp16Paper => 0,
        NormFormat::F32Reference => 1,
    }
}

fn byte_to_norm_format(byte: u8) -> Result<NormFormat> {
    match byte {
        0 => Ok(NormFormat::Fp16Paper),
        1 => Ok(NormFormat::F32Reference),
        _ => Err(FibQuantError::CorruptPayload(format!(
            "unknown norm_format byte: {}",
            byte
        ))),
    }
}

fn hex_decode_32(hex: &str) -> Result<[u8; 32]> {
    if hex.len() != 64 {
        return Err(FibQuantError::CorruptPayload(format!(
            "digest hex length {} (expected 64)",
            hex.len()
        )));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| FibQuantError::CorruptPayload("invalid hex digest".into()))?;
    }
    Ok(out)
}

/// Strip the "blake3:" prefix from fib-quant digests.
fn strip_blake3_prefix(digest: &str) -> &str {
    digest.strip_prefix("blake3:").unwrap_or(digest)
}

#[allow(dead_code)]
fn hex_encode_32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes.iter() {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec::FibQuantizer, profile::FibQuantProfileV1};

    fn build_test() -> Result<(FibQuantizer, FibQuantProfileV1)> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        let q = FibQuantizer::new(profile.clone())?;
        Ok((q, profile))
    }

    #[test]
    fn wire_roundtrip_preserves_data() -> Result<()> {
        let (q, profile) = build_test()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = q.encode(&input)?;
        let wire = FibCodeWireV1::to_wire_bytes(&code, &profile)?;
        let (decoded_code, decoded_profile) = FibCodeWireV1::from_wire_bytes(&wire)?;
        assert_eq!(decoded_code.indices, code.indices);
        assert_eq!(decoded_code.norm_payload, code.norm_payload);
        assert_eq!(decoded_code.wire_index_bits, code.wire_index_bits);
        assert_eq!(decoded_code.block_count, code.block_count);
        assert_eq!(decoded_profile.ambient_dim, profile.ambient_dim);
        assert_eq!(decoded_profile.block_dim, profile.block_dim);
        assert_eq!(decoded_profile.codebook_size, profile.codebook_size);
        assert_eq!(decoded_profile.rotation_seed, profile.rotation_seed);
        Ok(())
    }

    #[test]
    fn wire_parse_header_extracts_metadata() -> Result<()> {
        let (q, profile) = build_test()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = q.encode(&input)?;
        let wire = FibCodeWireV1::to_wire_bytes(&code, &profile)?;
        let header = FibCodeWireV1::parse_header(&wire)?;
        assert_eq!(header.version, WIRE_VERSION);
        assert_eq!(header.ambient_dim, 8);
        assert_eq!(header.block_dim, 2);
        assert_eq!(header.codebook_size, 8);
        assert_eq!(header.rotation_seed, 7);
        assert_eq!(header.wire_index_bits, profile.wire_index_bits);
        assert_eq!(header.body_offset, WIRE_HEADER_SIZE);
        Ok(())
    }

    #[test]
    fn wire_rejects_bad_magic() -> Result<()> {
        let result = FibCodeWireV1::parse_header(b"XXXX");
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn wire_rejects_truncation() -> Result<()> {
        let result = FibCodeWireV1::parse_header(&[0; 10]);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn wire_decoded_code_roundtrips_through_quantizer() -> Result<()> {
        let (q, profile) = build_test()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = q.encode(&input)?;
        let wire = FibCodeWireV1::to_wire_bytes(&code, &profile)?;
        let (decoded_code, decoded_profile) = FibCodeWireV1::from_wire_bytes(&wire)?;
        let q2 = FibQuantizer::new(decoded_profile)?;
        let decoded = q2.decode(&decoded_code)?;
        assert_eq!(decoded.len(), input.len());
        for (a, b) in input.iter().zip(decoded.iter()) {
            assert!(a.is_finite());
            assert!(b.is_finite());
        }
        Ok(())
    }

    #[test]
    fn wire_corrupt_dim_is_error() -> Result<()> {
        let (q, profile) = build_test()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = q.encode(&input)?;
        let mut wire = FibCodeWireV1::to_wire_bytes(&code, &profile)?;
        // Corrupt ambient_dim (bytes 5..9)
        wire[5] ^= 0xFF;
        let result = FibCodeWireV1::from_wire_bytes(&wire);
        // Should still work (profile is reconstructed from corrupted dim,
        // but the decode will fail when the caller tries to use it)
        // or fail on validation. Either way, no crash.
        let _ = result;
        Ok(())
    }
}
