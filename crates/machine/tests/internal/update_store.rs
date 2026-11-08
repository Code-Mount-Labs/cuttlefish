#![allow(clippy::expect_used)]
use super::*;
#[test]
fn journal_is_private_atomic_and_lock_is_released_when_owner_dies() {
    let (temp, manager) = super::super::tests::fixture();
    let store = manager.store().expect("store");
    assert!(store.read().expect("empty").job.is_none());
    let guard = store.lock().expect("lock").expect("acquired");
    assert!(store.lock().expect("busy").is_none());
    let inherited = guard.0.try_clone().expect("inherited descriptor");
    drop(guard);
    assert!(
        store
            .lock()
            .expect("released despite inherited descriptor")
            .is_some()
    );
    drop(inherited);
    store
        .save_job(super::super::tests::job(super::super::Phase::Queued))
        .expect("job");
    let reopened = Store::open(&manager.directory).expect("reopen");
    assert_eq!(
        reopened.read().expect("saved").job.expect("job").phase,
        super::super::Phase::Queued
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.directory.join("status.json"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&store.directory)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    std::fs::write(store.directory.join("status.json"), b"broken").expect("corrupt");
    assert!(store.read().is_err());
    std::fs::remove_file(store.directory.join("status.json")).expect("remove");
    std::fs::create_dir(store.directory.join("status.json")).expect("directory");
    assert!(store.read().is_err());
    assert!(store.write(&Record::default()).is_err());
    assert!(Store::open(&temp.path().join("missing")).is_err());
    assert!(lock(&store.directory).is_err());
}
#[cfg(unix)]
#[test]
fn update_paths_cannot_redirect_to_credentials() {
    let (temp, manager) = super::super::tests::fixture();
    let secret = temp.path().join("secret");
    std::fs::write(&secret, b"untouched").expect("secret");
    std::os::unix::fs::symlink(&secret, manager.directory.join("updates")).expect("symlink");
    assert!(manager.store().is_err());
    std::fs::remove_file(manager.directory.join("updates")).expect("remove");
    let store = manager.store().expect("store");
    for name in ["status.json", "worker.lock"] {
        let path = store.directory.join(name);
        std::os::unix::fs::symlink(&secret, &path).expect("symlink");
        assert!(reject_link(&path).is_err());
        std::fs::remove_file(path).expect("remove");
    }
    assert_eq!(std::fs::read(secret).expect("secret"), b"untouched");
}

#[test]
fn unsupported_file_lock_fails_closed() {
    let temp = tempfile::tempdir().expect("temp");
    assert!(
        lock_with(&temp.path().join("lock"), |_| Err(
            std::fs::TryLockError::Error(std::io::Error::other("locking unsupported"))
        ))
        .is_err()
    );
}

#[test]
fn directory_encoding_and_durable_write_failures_are_reported() {
    let (temp, manager) = super::super::tests::fixture();
    std::fs::write(manager.directory.join("updates"), b"blocked").expect("block directory");
    assert!(manager.store().is_err());
    std::fs::remove_file(manager.directory.join("updates")).expect("unblock");
    assert!(
        Store::open_with(&manager.directory, |_| Err(std::io::Error::other(
            "permissions unavailable"
        )))
        .is_err()
    );
    assert!(secure_directory(&temp.path().join("missing")).is_err());
    let mut store = manager.store().expect("store");
    store.encode = |_| {
        Err(serde_json::Error::io(std::io::Error::other(
            "encode unavailable",
        )))
    };
    assert!(store.write(&Record::default()).is_err());
    store.encode = serde_json::to_vec;
    store.files.sync = |_| Err(std::io::Error::other("disk unavailable"));
    assert!(store.write(&Record::default()).is_err());
    let path = store.directory.join("status.json");
    std::os::unix::fs::symlink("/missing", &path).expect("link");
    assert!(store.read().is_err());
    assert!(store.write(&Record::default()).is_err());
    assert!(
        store
            .save_job(super::super::tests::job(super::super::Phase::Queued))
            .is_err()
    );
}

#[test]
fn symlink_lock_never_opens_the_target() {
    let temp = tempfile::tempdir().expect("temp");
    let path = temp.path().join("lock");
    std::os::unix::fs::symlink("/missing", &path).expect("link");
    assert!(lock(&path).is_err());
}
