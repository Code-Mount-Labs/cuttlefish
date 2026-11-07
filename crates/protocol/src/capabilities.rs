//! Explicit capability declarations, independent of an operating-system guess.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A capability name must match `[a-z][a-z0-9._-]{0,63}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("capability must match [a-z][a-z0-9._-]{{0,63}}")]
pub struct InvalidCapability;

/// A canonical, case-sensitive capability name such as `build.ios`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Capability(String);

impl Capability {
    /// Native orchestration capabilities present in every super-agent runtime.
    #[must_use]
    pub fn native() -> std::collections::BTreeSet<Self> {
        ["agent.plan", "agent.delegate", "agent.learn"]
            .into_iter()
            .map(|name| Self(name.to_owned()))
            .collect()
    }
    /// Borrow the canonical name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Capability {
    type Error = InvalidCapability;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid_start = value.as_bytes().first().is_some_and(u8::is_ascii_lowercase);
        let valid_body = value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !valid_start || value.len() > 64 || !valid_body {
            return Err(InvalidCapability);
        }
        Ok(Self(value))
    }
}

impl From<Capability> for String {
    fn from(value: Capability) -> Self {
        value.0
    }
}

/// Supported native runtime platforms. Capabilities still require explicit checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Apple macOS.
    MacOs,
    /// Microsoft Windows.
    Windows,
    /// Linux distributions.
    Linux,
}
