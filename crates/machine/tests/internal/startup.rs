#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{Backend, initialize};
#[tokio::test]
async fn startup_rejects_bad_paths_permissions_corruption_and_failed_recovery_writes() {
    let backend = Arc::new(Backend::default());
    for blocker in ["", "prompts", "setup.lock", "setup.sqlite"] {
        let parent = tempfile::tempdir().expect("directory");
        let directory = parent.path().join("state");
        if blocker.is_empty() {
            std::fs::write(&directory, b"file").expect("file");
        } else {
            std::fs::create_dir(&directory).expect("directory");
            if blocker == "prompts" {
                std::fs::write(directory.join(blocker), b"file").expect("file");
            } else {
                std::fs::create_dir(directory.join(blocker)).expect("blocker");
            }
        }
        assert!(
            Machine::open(&directory, backend.clone()).await.is_err(),
            "{blocker}"
        );
    }
    let directory = tempfile::tempdir().expect("directory");
    assert!(
        Machine::open_with(directory.path(), backend.clone(), |_, _| Err(
            std::io::Error::other("permission failure")
        ))
        .await
        .is_err()
    );
    assert!(
        Machine::open_with(directory.path(), backend.clone(), |path, dir| if dir {
            protect(path, dir)
        } else {
            Err(std::io::Error::other("file permission failure"))
        })
        .await
        .is_err()
    );
    std::fs::write(directory.path().join("setup.sqlite"), b"corrupt database").expect("corrupt");
    assert!(
        Machine::open(directory.path(), backend.clone())
            .await
            .is_err()
    );
    let directory = tempfile::tempdir().expect("directory");
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("machine");
    initialize(&machine).await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=x'00'")
        .execute(&machine.pool)
        .await
        .expect("checksum");
    machine.close().await;
    drop(machine);
    assert!(
        Machine::open(directory.path(), backend.clone())
            .await
            .is_err()
    );
    let directory = tempfile::tempdir().expect("directory");
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("machine");
    sqlx::raw_sql("UPDATE resources SET state='ready'; CREATE TRIGGER fail BEFORE UPDATE ON resources BEGIN SELECT RAISE(ABORT,'injected'); END").execute(&machine.pool).await.expect("trigger");
    machine.close().await;
    drop(machine);
    assert!(Machine::open(directory.path(), backend).await.is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn symlinks_and_unresolvable_paths_cannot_redirect_private_state() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().expect("directory");
    let target = directory.path().join("target");
    std::fs::create_dir(&target).expect("target");
    let link = directory.path().join("link");
    symlink(&target, &link).expect("link");
    assert!(
        Machine::open(&link, Arc::new(Backend::default()))
            .await
            .is_err()
    );
    for name in ["setup.lock", "setup.sqlite"] {
        let path = target.join(name);
        symlink(directory.path().join("outside"), &path).expect("link");
        assert!(
            Machine::open(&target, Arc::new(Backend::default()))
                .await
                .is_err()
        );
        std::fs::remove_file(path).expect("unlink");
    }
    let recursive = directory.path().join("loop");
    symlink(&recursive, &recursive).expect("loop");
    assert!(reject_symlink(&recursive.join("child")).is_err());
    assert!(protect(&directory.path().join("missing"), false).is_err());
}
