#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id};
use orchestrator_protocol::execution::LocalProvider;
#[tokio::test]
async fn configurations_are_immutable_persistent_and_cannot_replace_used_workspaces() {
    let f = Fixture::new().await;
    let directory = f
        .journal
        .path()
        .parent()
        .expect("parent")
        .canonicalize()
        .expect("canonical");
    let prompt = directory.join("prompt.md");
    std::fs::write(&prompt, "test").expect("prompt");
    let config = ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Codex workspace".into(),
            provider: LocalProvider::Codex,
            allow_edits: true,
        },
        executable: "/bin/cat".into(),
        directory: directory.clone(),
        instructions: prompt,
    };
    f.journal
        .configure_execution(&config)
        .await
        .expect("configure");
    f.journal
        .configure_execution(&config)
        .await
        .expect("idempotent");
    assert_eq!(
        f.journal.executions().await.expect("list"),
        vec![config.clone()]
    );
    assert_eq!(
        f.journal.execution_config(id(4)).await.expect("absent"),
        None
    );
    f.registered().await;
    assert_eq!(
        f.journal.execution_config(id(4)).await.expect("resolve"),
        Some(config.clone())
    );
    let mut changed = config.clone();
    changed.workspace.allow_edits = false;
    assert!(f.journal.configure_execution(&changed).await.is_err());
    let pool = f.raw().await;
    sqlx::query("DELETE FROM workspace_providers")
        .execute(&pool)
        .await
        .expect("delete");
    assert!(f.journal.configure_execution(&config).await.is_err());
    for name in [String::new(), "x".repeat(129)] {
        changed.workspace.name = name;
        assert!(f.journal.configure_execution(&changed).await.is_err());
    }
    changed = config.clone();
    changed.executable = "relative".into();
    assert!(f.journal.configure_execution(&changed).await.is_err());
    changed.executable = directory.join("missing");
    assert!(f.journal.configure_execution(&changed).await.is_err());
    changed = config.clone();
    changed.instructions = directory.join("missing");
    assert!(f.journal.configure_execution(&changed).await.is_err());
    changed = config.clone();
    changed.directory = directory.join("missing");
    assert!(f.journal.configure_execution(&changed).await.is_err());
    check_directory(&config, &directory).expect("valid");
    assert!(check_directory(&config, &std::env::temp_dir()).is_err());
    assert!(check_directory(&changed, &directory).is_err());
    assert!(executable("cat", std::ffi::OsStr::new("/no-such-dir:/bin")).is_some());
    assert!(executable("no-such-provider", std::ffi::OsStr::new("/bin")).is_none());
    pool.close().await;
    f.journal.close().await;
}

fn configured(f: &Fixture) -> ExecutionConfig {
    let directory = f
        .journal
        .path()
        .parent()
        .expect("parent")
        .canonicalize()
        .expect("canonical");
    let instructions = directory.join("prompt.md");
    std::fs::write(&instructions, "prompt").expect("write");
    ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Code".into(),
            provider: LocalProvider::Claude,
            allow_edits: false,
        },
        executable: "/bin/cat".into(),
        directory,
        instructions,
    }
}
#[tokio::test]
async fn local_provider_journal_faults_do_not_replace_or_partially_save_configuration() {
    for fault in [
        "ALTER TABLE workspace_providers RENAME COLUMN configuration TO broken",
        "ALTER TABLE local_sessions RENAME COLUMN workspace_id TO broken",
        "CREATE TRIGGER fault BEFORE INSERT ON workspace_mounts BEGIN SELECT RAISE(ABORT,'mount failed'); END",
        "CREATE TRIGGER fault BEFORE INSERT ON workspace_providers BEGIN SELECT RAISE(ABORT,'config failed'); END",
        "CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER INSERT ON workspace_providers BEGIN INSERT INTO fault_child VALUES(1); END",
    ] {
        let f = Fixture::new().await;
        let config = configured(&f);
        let pool = f.raw().await;
        sqlx::raw_sql(fault).execute(&pool).await.expect("fault");
        assert!(f.journal.configure_execution(&config).await.is_err());
        pool.close().await;
        f.journal.close().await;
    }
    let f = Fixture::new().await;
    let mut config = configured(&f);
    let different = config.directory.join("different");
    std::fs::create_dir(&different).expect("directory");
    config.directory = different;
    assert!(f.journal.configure_execution(&config).await.is_err());
    f.journal.close().await;
    assert!(f.journal.configure_execution(&config).await.is_err());
    assert!(f.journal.executions().await.is_err());
    assert!(f.journal.execution_config(id(4)).await.is_err());
    let token = "a".repeat(64).try_into().expect("token");
    let remote = crate::sync::Remote::new("http://127.0.0.1:1", &token).expect("remote");
    assert!(remote.advertise_providers(&f.journal).await.is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn workspace_paths_reject_symlinks_non_utf8_and_removed_directories() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let f = Fixture::new().await;
    let mut config = configured(&f);
    let directory = config.directory.clone();
    let alias = directory.join("alias");
    symlink(&directory, &alias).expect("symlink");
    config.directory = alias;
    assert!(f.journal.configure_execution(&config).await.is_err());
    assert!(check_directory(&config, &directory).is_err());
    let invalid = directory.join(std::ffi::OsString::from_vec(vec![255]));
    config.directory = invalid;
    assert!(f.journal.configure_execution(&config).await.is_err());
    config.directory = directory.clone();
    assert!(check_directory(&config, &directory.join("missing")).is_err());
}
