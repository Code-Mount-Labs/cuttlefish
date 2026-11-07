use super::*;
use crate::tests::{Fixture, credentials};
#[tokio::test]
async fn cancelled_password_workers_and_hash_errors_never_create_sessions() {
    let mut f = Fixture::new().await;
    f.account("owner").await;
    f.platform.passwords.hash_password = |_| Err(Error::Password);
    assert!(matches!(
        f.platform.register_account(credentials("other")).await,
        Err(Error::Password)
    ));
    f.platform.passwords.hash_worker = |_| {
        let job = tokio::spawn(std::future::pending());
        job.abort();
        job
    };
    assert!(matches!(
        f.platform.register_account(credentials("other")).await,
        Err(Error::Worker(_))
    ));
    f.platform.passwords.verify_worker = |_| {
        let job = tokio::spawn(std::future::pending());
        job.abort();
        job
    };
    assert!(matches!(
        f.platform.login(credentials("owner")).await,
        Err(Error::Worker(_))
    ));
    assert!(matches!(
        finish_hash(Err(argon2::password_hash::Error::RngFailure)),
        Err(Error::Password)
    ));
    f.close().await;
}
