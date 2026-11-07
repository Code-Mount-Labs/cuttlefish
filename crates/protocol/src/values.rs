//! Validated scalar values; constructors and deserialization share invariants.

use std::{fmt, num::NonZeroU64};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest accepted message body in UTF-8 bytes.
pub const MAX_MESSAGE_BYTES: usize = 65_536;

/// A scalar did not satisfy the protocol contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValueError {
    /// Message text contained only whitespace or was empty.
    #[error("message text must contain a non-whitespace character")]
    EmptyText,
    /// Message text exceeded the documented byte limit.
    #[error("message text exceeds {MAX_MESSAGE_BYTES} UTF-8 bytes")]
    TextTooLong,
    /// Epoch zero is reserved for an unassigned resource.
    #[error("ownership epochs start at 1")]
    ZeroEpoch,
    /// An ownership epoch cannot be incremented without wrapping.
    #[error("ownership epoch exhausted")]
    EpochExhausted,
    /// This executable cannot interpret the supplied wire version.
    #[error("unsupported protocol version: {0}")]
    UnsupportedVersion(u16),
}

/// Nonempty, bounded text. Whitespace and Unicode are preserved exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MessageText(String);

impl MessageText {
    /// Borrow the original, validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MessageText {
    type Error = ValueError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > MAX_MESSAGE_BYTES {
            return Err(ValueError::TextTooLong);
        }
        if value.trim().is_empty() {
            return Err(ValueError::EmptyText);
        }
        Ok(Self(value))
    }
}

impl From<MessageText> for String {
    fn from(value: MessageText) -> Self {
        value.0
    }
}

/// A monotonically increasing ownership version, distinct from a retry count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Epoch(NonZeroU64);

impl Epoch {
    /// The first assignment of a resource.
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    /// Read the numeric epoch.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Obtain the next assignment version, without wrapping.
    ///
    /// # Errors
    /// Returns [`ValueError::EpochExhausted`] at `u64::MAX`.
    pub fn next(self) -> Result<Self, ValueError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(ValueError::EpochExhausted)
    }
}

impl TryFrom<u64> for Epoch {
    type Error = ValueError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ValueError::ZeroEpoch)
    }
}

impl From<Epoch> for u64 {
    fn from(value: Epoch) -> Self {
        value.get()
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}

/// The supported wire version. Unknown versions fail closed at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct ProtocolVersion;

impl ProtocolVersion {
    /// Version implemented by these contracts.
    pub const V1: Self = Self;
}

impl TryFrom<u16> for ProtocolVersion {
    type Error = ValueError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self::V1)
        } else {
            Err(ValueError::UnsupportedVersion(value))
        }
    }
}

impl From<ProtocolVersion> for u16 {
    fn from(_: ProtocolVersion) -> Self {
        1
    }
}
