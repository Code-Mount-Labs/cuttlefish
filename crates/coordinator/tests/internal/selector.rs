#![allow(clippy::expect_used)]
use super::*;

#[test]
fn invalid_configuration_and_response_are_rejected() {
    for endpoint in [
        "invalid",
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com?q=1",
        "https://example.com#fragment",
    ] {
        assert!(LlmSelector::new(endpoint, "secret", "configured-model").is_err());
    }
    assert!(LlmSelector::new("https://example.com", "", "model").is_err());
    assert!(LlmSelector::new("https://example.com", "key", " ").is_err());
    assert!(LlmSelector::new("https://example.com", "key", &"m".repeat(129)).is_err());
    assert!(LlmSelector::new("https://example.com", "key\n", "model").is_err());
    assert!(
        LlmSelector::with_builder(
            "https://example.com",
            "key",
            "model",
            Client::builder().user_agent("\n")
        )
        .is_err()
    );
    let selector =
        LlmSelector::new("https://example.com", "secret-key", "model").expect("valid config");
    assert!(!format!("{selector:?}").contains("secret-key"));
    for value in [
        json!({}),
        json!({"status":"completed"}),
        json!({"status":"completed","output":[]}),
        json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"bad JSON"}]}]}),
    ] {
        assert!(parse_response(&value).is_err());
    }
}
