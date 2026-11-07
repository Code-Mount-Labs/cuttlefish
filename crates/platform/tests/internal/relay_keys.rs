#![allow(clippy::expect_used)]
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
const PUBLIC: &str = include_str!("../fixtures/public-test-key.txt");
const PRIVATE: &str = include_str!("../fixtures/public-test-key.pem");
#[test]
fn malformed_signers_mismatched_keys_and_overflow_cannot_issue_tokens() {
    let mut keys = Keys::new(
        "https://issuer.test",
        "https://relay.test",
        PUBLIC,
        Some(PRIVATE),
    )
    .expect("keys");
    assert!(
        keys.issue(Uuid::new_v4(), Uuid::new_v4(), u64::MAX)
            .is_err()
    );
    keys.encoding = Some(EncodingKey::from_ed_der(b"invalid"));
    assert!(keys.issue(Uuid::new_v4(), Uuid::new_v4(), 1).is_err());
    let mut der = vec![
        0x30, 0x2d, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x21, 0x04,
        0x1f,
    ];
    der.extend_from_slice(&[1; 31]);
    let invalid = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
        STANDARD.encode(der)
    );
    assert!(
        Keys::new(
            "https://issuer.test",
            "https://relay.test",
            PUBLIC,
            Some(&invalid)
        )
        .is_err()
    );
    let other = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1; 32]);
    assert!(
        Keys::new(
            "https://issuer.test",
            "https://relay.test",
            &other,
            Some(PRIVATE)
        )
        .is_err()
    );
}
