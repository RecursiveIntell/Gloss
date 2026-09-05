#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DType {
    F32,
    F16,
    BF16,
    I8,
    U8,
    PackedBits,
}

impl DType {
    pub fn exact_byte_width(self) -> Option<u64> {
        match self {
            Self::F32 => Some(4),
            Self::F16 | Self::BF16 => Some(2),
            Self::I8 | Self::U8 => Some(1),
            Self::PackedBits => None,
        }
    }
}
