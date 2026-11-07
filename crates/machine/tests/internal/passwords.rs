#![allow(clippy::expect_used)]
use super::*;
#[tokio::test]
async fn cancelled_workers_and_failed_hashing_never_create_credentials() {
    let permit = || async {
        std::sync::Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .expect("permit")
    };
    let mut service = Passwords {
        hash_password: |_| Err(Error::Dependency("password_hashing_failed")),
        ..Passwords::default()
    };
    let input = || Credentials {
        username: "operator".into(),
        password: "long-enough-password".into(),
    };
    assert!(service.hash(input(), permit().await).await.is_err());
    service.hash_worker = |_| {
        let task = tokio::spawn(std::future::pending());
        task.abort();
        task
    };
    assert!(matches!(
        service.hash(input(), permit().await).await,
        Err(Error::Worker(_))
    ));
    service.verify_worker = |_| {
        let task = tokio::spawn(std::future::pending());
        task.abort();
        task
    };
    assert!(matches!(
        service
            .verify("password".into(), "hash".into(), permit().await)
            .await,
        Err(Error::Worker(_))
    ));
    assert!(finish_hash(Err(argon2::password_hash::Error::RngFailure)).is_err());
}
