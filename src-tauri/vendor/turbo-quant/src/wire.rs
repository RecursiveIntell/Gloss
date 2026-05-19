//! Deterministic compact wire encoding for [`TurboCode`].
//!
//! The wire bytes are a derived acceleration artifact. They are bound to the
//! quantizer profile on decode and are never authoritative over raw f32 vectors.

use crate::{
    bitpack,
    error::{Result, TurboQuantError},
    polar::PolarCode,
    qjl::QjlSketch,
    rotation::RotationKind,
    turbo::{TurboCode, TurboMode, TurboQuantizer},
};

/// Magic bytes for TurboCode wire format v1.
pub const TURBO_CODE_WIRE_MAGIC: &[u8; 4] = b"TQW1";

const VERSION: u16 = 1;
const VARIANT_TURBO_CODE: u8 = 1;

/// Encoder/decoder for TurboCode wire format v1.
pub struct TurboCodeWireV1;

impl TurboCodeWireV1 {
    /// Encode a validated TurboCode using the supplied quantizer profile.
    pub fn encode(code: &TurboCode, profile: &TurboQuantizer) -> Result<Vec<u8>> {
        code.validate_for(
            profile.dim(),
            profile.bits(),
            profile.projections(),
            profile.mode(),
        )?;

        let dim = checked_u32(profile.dim(), "dimension")?;
        let polar_bits = code.polar_code.bits;
        let qjl_projections = checked_u32(profile.projections(), "projection count")?;
        let polar_block_count = checked_u32(code.polar_code.radii.len(), "polar block count")?;
        let qjl_sign_count = checked_u32(
            code.residual_sketch
                .as_ref()
                .map_or(0, |sketch| sketch.projections),
            "qjl sign count",
        )?;
        let payload_len = checked_u64(
            code.polar_code.radii.len() * 4
                + code.polar_code.packed_angle_indices.len()
                + code
                    .residual_sketch
                    .as_ref()
                    .map_or(0, QjlSketch::encoded_bytes),
            "payload length",
        )?;

        let mut bytes = Vec::with_capacity(42 + payload_len as usize);
        bytes.extend_from_slice(TURBO_CODE_WIRE_MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&rotation_flag(profile.rotation_kind()).to_le_bytes());
        bytes.push(VARIANT_TURBO_CODE);
        bytes.push(0);
        bytes.extend_from_slice(&dim.to_le_bytes());
        bytes.push(polar_bits);
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes.extend_from_slice(&qjl_projections.to_le_bytes());
        bytes.extend_from_slice(&profile.seed().to_le_bytes());
        bytes.extend_from_slice(&polar_block_count.to_le_bytes());
        bytes.extend_from_slice(&qjl_sign_count.to_le_bytes());
        bytes.extend_from_slice(&payload_len.to_le_bytes());

        for radius in &code.polar_code.radii {
            bytes.extend_from_slice(&radius.to_le_bytes());
        }
        bytes.extend_from_slice(&code.polar_code.packed_angle_indices);
        if let Some(sketch) = &code.residual_sketch {
            bytes.extend_from_slice(&sketch.packed_signs);
            bytes.extend_from_slice(&sketch.norm.to_le_bytes());
        }
        Ok(bytes)
    }

    /// Decode and validate TurboCode wire bytes against the supplied profile.
    pub fn decode(bytes: &[u8], profile: &TurboQuantizer) -> Result<TurboCode> {
        let mut cursor = WireCursor::new(bytes);
        if cursor.read_exact(TURBO_CODE_WIRE_MAGIC.len())? != TURBO_CODE_WIRE_MAGIC {
            return Err(TurboQuantError::MalformedCode {
                reason: "wrong TurboQuant wire magic".into(),
            });
        }
        let version = cursor.read_u16()?;
        if version != VERSION {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("unsupported TurboQuant wire version {version}"),
            });
        }
        let wire_rotation_flag = cursor.read_u16()?;
        let expected_rotation_flag = rotation_flag(profile.rotation_kind());
        if wire_rotation_flag != expected_rotation_flag {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "wire rotation flag {wire_rotation_flag} does not match quantizer profile flag {expected_rotation_flag}"
                ),
            });
        }
        let variant = cursor.read_u8()?;
        if variant != VARIANT_TURBO_CODE {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("unsupported TurboQuant wire variant {variant}"),
            });
        }
        let reserved = cursor.read_u8()?;
        if reserved != 0 {
            return Err(TurboQuantError::MalformedCode {
                reason: "nonzero TurboQuant wire reserved byte".into(),
            });
        }

        let dim = cursor.read_u32()? as usize;
        let polar_bits = cursor.read_u8()?;
        let reserved2 = cursor.read_exact(3)?;
        if reserved2 != [0, 0, 0] {
            return Err(TurboQuantError::MalformedCode {
                reason: "nonzero TurboQuant wire reserved bytes".into(),
            });
        }
        let qjl_projections = cursor.read_u32()? as usize;
        let seed = cursor.read_u64()?;
        let polar_block_count = cursor.read_u32()? as usize;
        let qjl_sign_count = cursor.read_u32()? as usize;
        let payload_len = cursor.read_u64()?;
        let payload_start = cursor.offset();

        let expected_polar_bits = match profile.mode() {
            TurboMode::PolarOnly => profile.bits(),
            TurboMode::PolarWithQjl => profile.bits() - 1,
        };
        if dim != profile.dim()
            || polar_bits != expected_polar_bits
            || qjl_projections != profile.projections()
        {
            return Err(TurboQuantError::MalformedCode {
                reason: "wire header does not match quantizer profile".into(),
            });
        }
        if seed != profile.seed() {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "wire seed {seed} does not match quantizer profile seed {}",
                    profile.seed()
                ),
            });
        }
        if polar_block_count != profile.dim() / 2 {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "wire polar block count {polar_block_count} does not match dimension {}",
                    profile.dim()
                ),
            });
        }
        let expected_qjl_sign_count = match profile.mode() {
            TurboMode::PolarOnly => 0,
            TurboMode::PolarWithQjl => profile.projections(),
        };
        if qjl_sign_count != expected_qjl_sign_count {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "wire sign count {qjl_sign_count} does not match expected {expected_qjl_sign_count}"
                ),
            });
        }
        let angle_bytes = bitpack::packed_len(polar_block_count, polar_bits)?;
        let sign_bytes = match profile.mode() {
            TurboMode::PolarOnly => 0,
            TurboMode::PolarWithQjl => profile.projections().div_ceil(8),
        };
        let residual_bytes = match profile.mode() {
            TurboMode::PolarOnly => 0,
            TurboMode::PolarWithQjl => sign_bytes + 4,
        };
        let expected_payload_len = checked_u64(
            polar_block_count * 4 + angle_bytes + residual_bytes,
            "expected payload length",
        )?;
        if payload_len != expected_payload_len {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "TurboQuant wire payload length {payload_len} does not match expected {expected_payload_len}"
                ),
            });
        }
        if payload_len > cursor.remaining_len() as u64 {
            return Err(TurboQuantError::MalformedCode {
                reason: "TurboQuant wire payload length exceeds remaining bytes".into(),
            });
        }

        let mut radii = Vec::with_capacity(polar_block_count);
        for _ in 0..polar_block_count {
            radii.push(cursor.read_f32()?);
        }
        let packed_angle_indices = cursor.read_exact(angle_bytes)?.to_vec();
        let residual_sketch = match profile.mode() {
            TurboMode::PolarOnly => None,
            TurboMode::PolarWithQjl => {
                let packed_signs = cursor.read_exact(sign_bytes)?.to_vec();
                let norm = cursor.read_f32()?;
                Some(QjlSketch {
                    dim: profile.dim(),
                    projections: profile.projections(),
                    packed_signs,
                    norm,
                })
            }
        };
        if cursor.offset() - payload_start != payload_len as usize {
            return Err(TurboQuantError::MalformedCode {
                reason: "TurboQuant wire payload length mismatch".into(),
            });
        }
        cursor.finish()?;

        let code = TurboCode {
            polar_code: PolarCode {
                dim: profile.dim(),
                bits: polar_bits,
                radii,
                packed_angle_indices,
            },
            residual_sketch,
        };
        code.validate_for(
            profile.dim(),
            profile.bits(),
            profile.projections(),
            profile.mode(),
        )?;
        Ok(code)
    }
}

fn checked_u32(value: usize, field: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| TurboQuantError::MalformedCode {
        reason: format!("{field} {value} does not fit u32 wire field"),
    })
}

fn checked_u64(value: usize, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| TurboQuantError::MalformedCode {
        reason: format!("{field} {value} does not fit u64 wire field"),
    })
}

fn rotation_flag(kind: RotationKind) -> u16 {
    match kind {
        RotationKind::Auto => 0,
        RotationKind::FastHadamard => 1,
        RotationKind::StoredQr => 2,
    }
}

struct WireCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> WireCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn offset(&self) -> usize {
        self.offset
    }

    fn remaining_len(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| TurboQuantError::MalformedCode {
                reason: "wire offset overflow".into(),
            })?;
        if end > self.bytes.len() {
            return Err(TurboQuantError::MalformedCode {
                reason: "truncated TurboQuant wire artifact".into(),
            });
        }
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16> {
        let bytes = self.read_exact(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_exact(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let bytes = self.read_exact(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_f32(&mut self) -> Result<f32> {
        let bytes = self.read_exact(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn finish(&self) -> Result<()> {
        if self.offset != self.bytes.len() {
            return Err(TurboQuantError::MalformedCode {
                reason: "trailing bytes in TurboQuant wire artifact".into(),
            });
        }
        Ok(())
    }
}
