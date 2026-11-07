//! Bounded file references. Content travels once through authenticated blob APIs.
use crate::ids::{AttachmentId, WorkspaceId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
/// Maximum file size supported by the initial attachment transport.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Each JSON upload stays below the relay's request ceiling.
pub const CHUNK_BYTES: usize = 192 * 1024;
/// Maximum files attached to one message or result.
pub const MAX_FILES: usize = 8;
/// Invalid or inconsistent file metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid attachment metadata or content")]
pub struct InvalidAttachment;
/// An immutable, workspace-scoped reference to uploaded content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Stable upload identity, also used for exact retry.
    pub id: AttachmentId,
    /// Workspace authorized to consume the file.
    pub workspace_id: WorkspaceId,
    /// Original display filename; never interpreted as a caller-supplied path.
    pub name: String,
    /// Display media type. Downloads are served as attachments.
    pub media_type: String,
    /// Exact byte count.
    pub size: u64,
    /// Lowercase SHA-256 of the complete content.
    pub sha256: String,
}
impl Attachment {
    /// Validate metadata before allocating or persisting content.
    ///
    /// # Errors
    /// Rejects oversized files, path-like names and malformed digests.
    pub fn validate(&self) -> Result<(), InvalidAttachment> {
        if self.size > MAX_FILE_BYTES
            || self.name.is_empty()
            || self.name.len() > 180
            || matches!(self.name.as_str(), "." | "..")
            || self
                .name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || self.media_type.is_empty()
            || self.media_type.len() > 128
            || !self.media_type.bytes().all(|b| b.is_ascii_graphic())
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidAttachment);
        }
        Ok(())
    }
    /// Number of chunks, including one empty chunk for an empty file.
    #[must_use]
    pub fn chunks(&self) -> u64 {
        self.size.div_ceil(CHUNK_BYTES as u64).max(1)
    }
    /// Check complete content against the immutable reference.
    ///
    /// # Errors
    /// Rejects any metadata, byte-count or checksum mismatch.
    pub fn verify(&self, data: &[u8]) -> Result<(), InvalidAttachment> {
        self.validate()?;
        if self.size != data.len() as u64 || self.sha256 != format!("{:x}", Sha256::digest(data)) {
            return Err(InvalidAttachment);
        }
        Ok(())
    }
}
/// A bounded, independently retryable chunk submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentChunk {
    /// Immutable metadata repeated so retries cannot change the file.
    pub attachment: Attachment,
    /// Zero-based position.
    pub index: u32,
    /// Standard base64 content; no data-URL prefix.
    pub data: String,
}
