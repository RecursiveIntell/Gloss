//! Feature-gated type aliases for stack-ids integration.
//!
//! When the `typed-ids` feature is active, `AgentId` is `stack_ids::EntityId`.
//! When inactive, `AgentId` is a local newtype wrapping `String`.

#[cfg(feature = "typed-ids")]
pub use stack_ids::EntityId as AgentId;

#[cfg(not(feature = "typed-ids"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

#[cfg(not(feature = "typed-ids"))]
impl AgentId {
    /// Create from any string-like value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if the inner string is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(not(feature = "typed-ids"))]
impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(not(feature = "typed-ids"))]
impl From<String> for AgentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[cfg(not(feature = "typed-ids"))]
impl From<&str> for AgentId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(not(feature = "typed-ids"))]
impl AsRef<str> for AgentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
