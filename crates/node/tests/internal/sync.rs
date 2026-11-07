#![allow(clippy::expect_used)]
use super::*;

#[test]
fn endpoint_validation_and_http_initialization_fail_closed() {
    let token = NodeToken::try_from("a".repeat(64)).expect("token");
    for origin in [
        "not a URL",
        "http://example.com",
        "ftp://127.0.0.1",
        "https://user:secret@example.com",
        "https://example.com/prefix",
        "https://example.com?query=1",
        "https://example.com#fragment",
    ] {
        assert!(Remote::new(origin, &token).is_err(), "{origin}");
    }
    for origin in ["https://example.com", "http://127.0.0.1", "http://[::1]"] {
        assert!(Remote::new(origin, &token).is_ok());
    }
    assert!(
        Remote::with_builder(
            "https://example.com",
            &token,
            Client::builder().user_agent("\n")
        )
        .is_err()
    );
}
