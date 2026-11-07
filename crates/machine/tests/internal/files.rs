#![allow(clippy::expect_used)]
use super::*;
fn failure() -> io::Error {
    io::Error::other("injected disk failure")
}
#[test]
fn each_disk_failure_is_reported_and_never_replaces_an_existing_file() {
    let dir = tempfile::tempdir().expect("directory");
    let target = dir.path().join("secret");
    let original = AtomicFiles::default();
    for operation in 0..5 {
        let mut files = original.clone();
        match operation {
            0 => files.create = |_| Err(failure()),
            1 => files.write = |_, _| Err(failure()),
            2 => files.sync = |_| Err(failure()),
            3 => files.persist = |_, _| Err(failure()),
            _ => files.sync_directory = |_| Err(failure()),
        }
        assert!(files.write_new(dir.path(), &target, b"secret").is_err());
        if target.exists() {
            std::fs::remove_file(&target).expect("remove committed file");
        }
    }
    original
        .write_new(dir.path(), &target, b"secret")
        .expect("durable write");
    assert!(
        original
            .write_new(dir.path(), &target, b"replacement")
            .is_err()
    );
    assert_eq!(std::fs::read(&target).expect("read"), b"secret");
    assert!((original.create)(&dir.path().join("absent")).is_err());
    #[cfg(unix)]
    {
        assert!((original.sync_directory)(&dir.path().join("absent")).is_err());
    }
}
