#![allow(clippy::expect_used)]
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn fixture() -> (tempfile::TempDir, Manager) {
    let temp = tempfile::tempdir().expect("fixture");
    let state = temp.path().join("state");
    let slot = temp.path().join("app/releases/old");
    std::fs::create_dir_all(&state).expect("state");
    std::fs::create_dir_all(&slot).expect("slot");
    std::fs::write(state.join("setup.sqlite"), b"unchanged database").expect("database");
    std::fs::write(slot.join("cuttlefish"), "#!/bin/sh\nexit 0\n").expect("executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            slot.join("cuttlefish"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("mode");
        std::os::unix::fs::symlink("releases/old", temp.path().join("app/current"))
            .expect("current");
    }
    std::fs::write(
        temp.path().join("app/.cuttlefish-layout"),
        b"cuttlefish-release-layout-v1\n",
    )
    .expect("marker");
    let mut manager = Manager::new(
        &state,
        slot.join("cuttlefish"),
        Some(temp.path().join("home")),
    );
    manager.current = "v0.2.0-alpha.6".into();
    manager.os = "macos";
    (temp, manager)
}
pub(super) fn service(manager: &Manager) -> PathBuf {
    use sha2::{Digest, Sha256};
    let canonical = std::fs::canonicalize(&manager.directory).expect("canonical");
    let hash = format!(
        "{:x}",
        Sha256::digest(canonical.as_os_str().as_encoded_bytes())
    );
    let folder = manager
        .home
        .as_ref()
        .expect("home")
        .join("Library/LaunchAgents");
    std::fs::create_dir_all(&folder).expect("folder");
    let path = folder.join(format!("com.cuttlefish.app.{}.plist", &hash[..32]));
    std::fs::write(&path, "<!-- Cuttlefish listener: 127.0.0.1:8123 -->\n").expect("service");
    path
}
pub(super) fn job(phase: Phase) -> Job {
    Job {
        id: Uuid::new_v4(),
        version: "v0.2.0-alpha.7".into(),
        restart_only: false,
        phase,
        error: None,
    }
}
pub(super) fn save(manager: &Manager, job: Job) {
    manager.store().expect("store").save_job(job).expect("save");
}
#[derive(Debug, Default)]
pub(super) struct Fake {
    installs: AtomicUsize,
    restarts: AtomicUsize,
    fail: bool,
    pub(super) lose_journal: bool,
}
impl worker::Operations for Fake {
    fn install<'a>(
        &'a self,
        root: &'a Path,
        version: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            self.installs.fetch_add(1, Ordering::SeqCst);
            if self.lose_journal {
                let status = root
                    .parent()
                    .expect("fixture root")
                    .join("state/updates/status.json");
                std::fs::remove_file(&status)?;
                std::fs::create_dir(status)?;
                return Err(Error::Dependency("disk lost"));
            }
            if self.fail {
                return Err(Error::Dependency("download_failed"));
            }
            std::fs::write(root.join("current/.release-version"), version)?;
            Ok(())
        })
    }
    fn restart<'a>(
        &'a self,
        _: &'a Manager,
        _: std::net::SocketAddr,
        _: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(Error::Dependency("restart_failed"))
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn interrupted_jobs_resume_once_without_touching_data() {
    for phase in [Phase::Queued, Phase::Downloading, Phase::Restarting] {
        let (_temp, manager) = fixture();
        service(&manager);
        let pending = job(phase);
        let id = pending.id;
        save(&manager, pending);
        let operations = Fake::default();
        worker::run(&manager, &operations).await.expect("recover");
        worker::run(&manager, &operations).await.expect("repeat");
        let result = manager.status().expect("status").job.expect("job");
        assert_eq!(result.id, id);
        assert_eq!(result.phase, Phase::Complete);
        assert_eq!(operations.restarts.load(Ordering::SeqCst), 1);
        assert_eq!(
            operations.installs.load(Ordering::SeqCst),
            usize::from(phase != Phase::Restarting)
        );
        assert_eq!(
            std::fs::read(manager.directory.join("setup.sqlite")).expect("db"),
            b"unchanged database"
        );
    }
}
#[tokio::test]
async fn failed_download_and_restart_are_retryable_with_stable_identity() {
    let (_temp, manager) = fixture();
    save(&manager, job(Phase::Queued));
    let id = manager.status().expect("status").job.expect("job").id;
    worker::run(
        &manager,
        &Fake {
            fail: true,
            ..Fake::default()
        },
    )
    .await
    .expect("failure recorded");
    assert_eq!(
        manager.status().expect("status").job.expect("job").phase,
        Phase::Failed
    );
    manager.enqueue(Action::Retry).expect("retry");
    let operations = Fake::default();
    worker::run(&manager, &operations).await.expect("install");
    assert_eq!(
        manager.status().expect("status").job.expect("job").phase,
        Phase::RestartRequired
    );
    service(&manager);
    manager.enqueue(Action::Retry).expect("retry");
    worker::run(
        &manager,
        &Fake {
            fail: true,
            ..Fake::default()
        },
    )
    .await
    .expect("restart failure");
    let failed = manager.status().expect("status").job.expect("job");
    assert_eq!(failed.error.as_deref(), Some("restart_failed"));
    manager.enqueue(Action::Retry).expect("retry");
    worker::run(&manager, &operations).await.expect("restart");
    let result = manager.status().expect("status").job.expect("job");
    assert_eq!(result.phase, Phase::Complete);
    assert_eq!(result.id, id);
    assert_eq!(operations.installs.load(Ordering::SeqCst), 1);
    assert!(result.error.is_none());
    assert!(manager.enqueue(Action::Retry).is_err());
}
#[tokio::test]
async fn concurrency_invalid_versions_and_corrupt_layouts_fail_closed() {
    let (_temp, manager) = fixture();
    let store = manager.store().expect("store");
    let lock = store.lock().expect("lock");
    manager.enqueue(Action::Restart).expect("busy");
    manager.check("invalid:").await.expect("busy");
    manager.resume().expect("busy");
    worker::run(&manager, &Fake::default()).await.expect("busy");
    drop(lock);
    assert!(manager.enqueue(Action::Retry).is_err());
    assert!(manager.enqueue(Action::Restart).is_err());
    assert!(manager.enqueue(Action::Check).is_err());
    assert!(
        manager
            .enqueue(Action::Install {
                version: "v0.2.0-alpha.7".into()
            })
            .is_err()
    );
    assert!(
        manager
            .enqueue(Action::Install {
                version: "bad".into()
            })
            .is_err()
    );
    store
        .write(&Record {
            available: Some("v0.2.0-alpha.7".into()),
            ..Record::default()
        })
        .expect("available");
    manager
        .enqueue(Action::Install {
            version: "v0.2.0-alpha.7".into(),
        })
        .expect("selected");
    let id = manager.status().expect("status").job.expect("job").id;
    manager.enqueue(Action::Restart).expect("already queued");
    manager.check("invalid:").await.expect("already queued");
    assert_eq!(manager.status().expect("status").job.expect("job").id, id);
    let root_lock =
        store::lock(&manager.root().expect("root").join("update.lock")).expect("root lock");
    worker::run(&manager, &Fake::default())
        .await
        .expect("failure recorded");
    assert_eq!(
        manager.status().expect("status").job.expect("job").phase,
        Phase::Failed
    );
    drop(root_lock);
    let mut invalid = job(Phase::Queued);
    invalid.version = "bad".into();
    save(&manager, invalid);
    worker::run(&manager, &Fake::default())
        .await
        .expect("invalid version recorded");
    save(&manager, job(Phase::Queued));
    std::fs::remove_file(manager.root().expect("root").join(".cuttlefish-layout"))
        .expect("remove marker");
    worker::run(&manager, &Fake::default())
        .await
        .expect("invalid layout recorded");
    assert!(!manager.status().expect("status").supported);
}
#[tokio::test]
async fn restart_only_skips_download_and_supervisor_resumes_pending_work() {
    let (_temp, manager) = fixture();
    service(&manager);
    manager.resume().expect("nothing to resume");
    manager.enqueue(Action::Restart).expect("restart");
    let operations = Fake::default();
    manager.resume().expect("spawn helper");
    worker::run(&manager, &operations).await.expect("restart");
    assert_eq!(operations.installs.load(Ordering::SeqCst), 0);
    assert_eq!(operations.restarts.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager.status().expect("status").job.expect("job").version,
        manager.current
    );
    for phase in [
        Phase::Queued,
        Phase::Downloading,
        Phase::Restarting,
        Phase::Complete,
        Phase::RestartRequired,
        Phase::Failed,
    ] {
        assert!(!phase.label().is_empty());
        assert_eq!(
            serde_json::from_value::<Phase>(serde_json::to_value(phase).expect("json"))
                .expect("phase"),
            phase
        );
    }
}
#[tokio::test]
async fn release_checks_are_bounded_and_persist_failures() {
    let (_temp, manager) = fixture();
    let server = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}", server.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        axum::serve(
            server,
            axum::Router::new().route("/", axum::routing::get(|| async { "[]" })),
        )
        .await
        .expect("serve");
    });
    manager.check(&url).await.expect("check");
    let status = manager.status().expect("status");
    assert!(status.checked);
    assert!(status.available_version.is_none());
    cli(&manager, true, &url, 0, Duration::ZERO)
        .await
        .expect("check CLI");
    cli(&manager, false, &url, 0, Duration::ZERO)
        .await
        .expect("current CLI");
    manager.check("bad:").await.expect("failure saved");
    assert!(manager.status().expect("status").check_error.is_some());
    assert!(
        cli(&manager, true, "bad:", 0, Duration::ZERO)
            .await
            .is_err()
    );
    manager.check(&url).await.expect("retry check");
    save(&manager, job(Phase::Queued));
    assert!(cli(&manager, false, &url, 1, Duration::ZERO).await.is_err());
    task.abort();
}

#[tokio::test]
async fn public_actions_and_cli_observe_detached_helper_results() {
    let (_temp, manager) = fixture();
    service(&manager);
    let status = manager
        .act(Action::Restart)
        .await
        .expect("restart accepted");
    assert_eq!(status.job.expect("job").phase, Phase::Queued);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let target = catalog::target(manager.os, std::env::consts::ARCH).expect("target");
    let releases = serde_json::json!([{"tag_name":"v0.2.0-alpha.7","draft":false,"assets":[{"name":format!("cuttlefish-v0.2.0-alpha.7-{target}.tar.gz")},{"name":"SHA256SUMS"}]}]);
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(move || {
                    let releases = releases.clone();
                    async { axum::Json(releases) }
                }),
            ),
        )
        .await
        .expect("serve");
    });
    for (initial, outcome) in [
        (None, Phase::Complete),
        (Some(Phase::Failed), Phase::Complete),
        (None, Phase::Failed),
    ] {
        manager
            .store()
            .expect("store")
            .write(&Record {
                job: initial.map(job),
                ..Record::default()
            })
            .expect("reset");
        let completed = serde_json::to_string(&Record {
            job: Some(job(outcome)),
            ..Record::default()
        })
        .expect("json");
        let script = format!(
            "#!/bin/sh\nsleep 0.05\ncat > \"$2/updates/done.json\" <<'JSON'\n{completed}\nJSON\nmv \"$2/updates/done.json\" \"$2/updates/status.json\"\n"
        );
        std::fs::write(&manager.executable, script).expect("helper");
        assert_eq!(
            cli(&manager, false, &url, 100, Duration::from_millis(10))
                .await
                .is_ok(),
            outcome == Phase::Complete
        );
    }
    task.abort();
}
#[test]
fn stale_or_invalid_available_versions_are_not_offered() {
    let (_temp, manager) = fixture();
    for (tag, shown) in [
        ("v0.2.0-alpha.7", true),
        ("v0.2.0-alpha.6", false),
        ("bad", false),
    ] {
        manager
            .store()
            .expect("store")
            .write(&Record {
                available: Some(tag.into()),
                ..Record::default()
            })
            .expect("candidate");
        assert_eq!(
            manager
                .status()
                .expect("status")
                .available_version
                .is_some(),
            shown
        );
    }
}

#[tokio::test]
async fn damaged_journals_and_lock_files_fail_closed_at_every_entrypoint() {
    for damage in ["state", "lock", "record"] {
        let (_temp, manager) = fixture();
        let store = manager.store().expect("store");
        match damage {
            "state" => {
                std::fs::remove_file(manager.directory.join("setup.sqlite")).expect("remove");
            }
            "lock" => std::fs::create_dir(store.directory.join("worker.lock")).expect("block lock"),
            _ => std::fs::write(store.directory.join("status.json"), b"corrupt").expect("corrupt"),
        }
        assert!(manager.check("bad:").await.is_err());
        assert!(manager.enqueue(Action::Restart).is_err());
        assert!(manager.resume().is_err());
        assert!(worker::run(&manager, &Fake::default()).await.is_err());
        if damage != "lock" {
            assert!(manager.status().is_err());
        }
    }
    let (_temp, mut manager) = fixture();
    manager.os = "unsupported";
    manager.check("bad:").await.expect("unsupported recorded");
    assert!(manager.status().expect("status").check_error.is_some());
    assert!(!manager.status().expect("status").supported);
    manager.os = "macos";
    let path = service(&manager);
    std::fs::write(path, "broken").expect("broken service");
    assert!(manager.status().is_err());
    assert!(manager.enqueue(Action::Restart).is_err());
    save(&manager, job(Phase::Restarting));
    worker::run(&manager, &Fake::default())
        .await
        .expect("failure recorded");
    assert_eq!(
        manager
            .store()
            .expect("store")
            .read()
            .expect("record")
            .job
            .expect("job")
            .phase,
        Phase::Failed
    );
}
#[tokio::test]
async fn helper_spawn_failures_leave_the_durable_request_recoverable() {
    let (_temp, mut manager) = fixture();
    let store = manager.store().expect("store");
    save(&manager, job(Phase::Queued));
    let log = store.directory.join("helper.log");
    std::os::unix::fs::symlink("/missing", &log).expect("link");
    assert!(manager.resume().is_err());
    std::fs::remove_file(&log).expect("remove");
    std::fs::create_dir(&log).expect("block log");
    assert!(manager.resume().is_err());
    std::fs::remove_dir(&log).expect("remove");
    manager.executable = manager.executable.with_file_name("missing");
    assert!(manager.resume().is_err());
    assert!(manager.act(Action::Restart).await.is_err());
    assert_eq!(
        store.read().expect("record").job.expect("job").phase,
        Phase::Queued
    );
}

#[test]
fn missing_process_path_prevents_updater_discovery() {
    assert!(
        discover(Path::new("/missing"), || Err(std::io::Error::other(
            "executable missing"
        )))
        .is_err()
    );
}

#[tokio::test]
async fn descriptor_failures_keep_request_pending_for_recovery() {
    let (_temp, mut manager) = fixture();
    save(&manager, job(Phase::Queued));
    manager.process_files.secure = |_| Err(std::io::Error::other("permissions unavailable"));
    assert!(manager.resume().is_err());
    manager.process_files = ProcessFiles::default();
    manager.process_files.duplicate = |_| Err(std::io::Error::other("descriptor unavailable"));
    assert!(manager.resume().is_err());
    // A request can be durably accepted before spawning fails; retries retain it.
    // Concurrent process creation can briefly inherit an OS-held lock. A busy
    // request is accepted without duplication; its retry must expose the fault.
    let mut failed = false;
    for _ in 0..50 {
        if manager.act(Action::Retry).await.is_err() {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(failed);
    assert!(
        cli(&manager, false, "invalid:", 0, Duration::ZERO)
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .status()
            .expect("status")
            .job
            .expect("pending")
            .phase,
        Phase::Queued
    );
}

struct FaultyControl {
    statuses: std::cell::RefCell<std::collections::VecDeque<Result<Status, Error>>>,
}
impl Control for FaultyControl {
    fn check<'a>(
        &'a self,
        _: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn status(&self) -> Result<Status, Error> {
        self.statuses
            .borrow_mut()
            .pop_front()
            .expect("status scripted")
    }
    fn enqueue(&self, _: Action) -> Result<(), Error> {
        Err(Error::Invalid("journal unavailable"))
    }
    fn resume(&self) -> Result<(), Error> {
        Ok(())
    }
}
#[tokio::test]
async fn cli_reports_concurrent_storage_loss_without_claiming_success() {
    let (_temp, manager) = fixture();
    let status = || manager.status().expect("status");
    let retry = Status {
        job: Some(job(Phase::Failed)),
        ..status()
    };
    let install = Status {
        available_version: Some("v0.2.0-alpha.7".into()),
        ..status()
    };
    let active = || Status {
        job: Some(job(Phase::Queued)),
        ..status()
    };
    let fail = || Err(Error::Invalid("storage lost"));
    for results in [
        vec![fail()],
        vec![Ok(retry)],
        vec![Ok(install)],
        vec![Ok(active()), fail()],
        vec![Ok(active()), Ok(status())],
    ] {
        let control = FaultyControl {
            statuses: std::cell::RefCell::new(results.into()),
        };
        assert!(
            cli(&control, false, "unused", 1, Duration::ZERO)
                .await
                .is_err()
        );
    }
}
