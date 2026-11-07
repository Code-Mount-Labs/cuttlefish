#![allow(clippy::expect_used)]
use super::*;
#[tokio::test]
async fn closing_storage_between_password_verification_and_session_creation_is_safe() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(
        dir.path(),
        std::sync::Arc::new(crate::tests::Backend::default()),
    )
    .await
    .expect("machine");
    machine.close().await;
    assert!(machine.issue_login_session("stale-hash").await.is_err());
}

#[path = "password_change.rs"]
mod password_change;
