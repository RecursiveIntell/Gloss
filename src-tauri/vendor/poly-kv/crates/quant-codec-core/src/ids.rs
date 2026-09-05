use crate::QuantCodecError;
use core::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, QuantCodecError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(QuantCodecError::EmptyIdentifier {
                        type_name: stringify!($name),
                    });
                }
                if value.trim() != value {
                    return Err(QuantCodecError::IdentifierWhitespace {
                        type_name: stringify!($name),
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0).finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(CodecId);
string_id!(ModelFingerprint);
string_id!(TokenizerFingerprint);
