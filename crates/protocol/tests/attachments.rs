//! File metadata and checksum contract tests.
#![allow(clippy::expect_used)]
use orchestrator_protocol::{
    attachments::{Attachment, CHUNK_BYTES, MAX_FILE_BYTES},
    ids::{AttachmentId, WorkspaceId},
};
use sha2::{Digest, Sha256};
fn file() -> Attachment {
    Attachment {
        id: AttachmentId::random(),
        workspace_id: WorkspaceId::try_from(uuid::Uuid::from_u128(5)).expect("id"),
        name: "report.txt".into(),
        media_type: "text/plain".into(),
        size: 3,
        sha256: format!("{:x}", Sha256::digest(b"abc")),
    }
}
#[test]
fn attachment_metadata_rejects_paths_bad_digests_and_unbounded_content() {
    let valid = file();
    valid.verify(b"abc").expect("content");
    assert_eq!(valid.chunks(), 1);
    for name in ["", ".", "..", "a/b", "a\\b", "a\nb"] {
        let mut bad = valid.clone();
        bad.name = name.into();
        assert!(bad.validate().is_err());
    }
    let mut bad = valid.clone();
    bad.name = "a".repeat(181);
    assert!(bad.validate().is_err());
    for mime in [String::new(), "text/plain\n".into(), "a".repeat(129)] {
        let mut bad = valid.clone();
        bad.media_type = mime;
        assert!(bad.validate().is_err());
    }
    for digest in ["a".repeat(63), "g".repeat(64), "A".repeat(64)] {
        let mut bad = valid.clone();
        bad.sha256 = digest;
        assert!(bad.validate().is_err());
    }
    assert!(valid.verify(b"ab").is_err());
    assert!(valid.verify(b"abd").is_err());
    bad = valid;
    bad.size = MAX_FILE_BYTES + 1;
    assert!(bad.validate().is_err());
    assert!(bad.verify(b"abc").is_err());
    bad.size = CHUNK_BYTES as u64 + 1;
    assert_eq!(bad.chunks(), 2);
    bad.size = 0;
    assert_eq!(bad.chunks(), 1);
}
