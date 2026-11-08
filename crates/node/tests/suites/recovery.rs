//! Recovery exercised through the public runtime and separately launched worker binary.
#![allow(clippy::expect_used)]
use crate::common::{Fixture, id, instance, start};
use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
use orchestrator_node::{
    execution::ExecutionConfig,
    local_provider::LocalAgent,
    model::{CommandStatus, SessionStatus},
    process::LocalLauncher,
    runtime::NodeRuntime,
    sync::Remote,
    worker,
};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    recovery::{RecoveryJob, RecoveryState, RecoveryView},
    routing::{DispatchBatch, DispatchPoll},
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
const BINARY: &str = env!("CARGO_BIN_EXE_orchestrator-node");
fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("mode");
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::process::Command::new(BINARY)
            .arg("stop")
            .arg("--state-dir")
            .arg(&self.0)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
async fn saved_work() -> (Fixture, ExecutionConfig, RecoveryJob) {
    let f = Fixture::new().await;
    let root = f.temp.path().canonicalize().expect("root");
    let directory = root.join("work");
    std::fs::create_dir(&directory).expect("work");
    f.journal.mount(id(5), &directory).await.expect("mount");
    let executable = root.join("provider");
    script(
        &executable,
        r#"input=$(cat)
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-0000-0000-000000000004"}'
printf 'run\n' >> invocations.txt
if [ ! -f completed.txt ]; then printf 'original work' > completed.txt; exit 1; fi
case " $* " in *" --resume 00000000-0000-0000-0000-000000000004 "*) ;; *) exit 2;; esac
case "$input" in *"not a new task"*) ;; *) exit 3;; esac
printf 'ready' > recovering.txt
while [ ! -f "$(dirname "$0")/continue" ]; do sleep 0.05; done
[ "$(cat completed.txt)" = 'original work' ] || exit 4
printf 'remaining work' > remaining.txt
printf '%s\n' '{"type":"result","session_id":"00000000-0000-0000-0000-000000000004","is_error":false,"subtype":"success","result":"Continued existing work"}'"#,
    );
    let instructions = root.join("instructions.md");
    std::fs::write(&instructions, "Test existing work recovery").expect("prompt");
    let config = ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Recovery".into(),
            provider: LocalProvider::Claude,
            allow_edits: true,
        },
        executable,
        directory,
        instructions,
    };
    f.journal
        .configure_execution(&config)
        .await
        .expect("config");
    f.accepted().await;
    let agent = LocalAgent::new(
        f.journal.clone(),
        instance().to_string(),
        config.clone(),
        BINARY.into(),
    );
    assert!(
        worker::run(&f.journal, id(4), &instance().to_string(), agent)
            .await
            .is_err()
    );
    assert_eq!(f.status(6).await, CommandStatus::Uncertain);
    let job = RecoveryJob {
        view: RecoveryView {
            id: id(100),
            command_id: id(6),
            state: RecoveryState::Inspecting,
            checkpoint: None,
            detail: None,
        },
        command: start(),
    };
    (f, config, job)
}
struct Server {
    job: Arc<Mutex<RecoveryJob>>,
    report: Arc<Mutex<Option<RecoveryView>>>,
    mode: Arc<AtomicU8>,
    task: tokio::task::JoinHandle<()>,
    remote: Remote,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(job: RecoveryJob) -> Server {
    let job = Arc::new(Mutex::new(job));
    let report = Arc::new(Mutex::new(None));
    let mode = Arc::new(AtomicU8::new(0));
    let current = job.clone();
    let saved = report.clone();
    let fail = mode.clone();
    let app = Router::new()
        .route(
            "/v1/dispatch/poll",
            post(move |Json(p): Json<DispatchPoll>| {
                let job = current.lock().expect("job").clone();
                async move {
                    Json(DispatchBatch {
                        acknowledged: p.accepted,
                        command: None,
                        recovery: Some(job),
                    })
                }
            }),
        )
        .route(
            "/v1/recovery/report",
            post(move |Json(view): Json<RecoveryView>| {
                let id = view.id;
                *saved.lock().expect("report") = Some(view);
                let failure = fail.load(Ordering::SeqCst);
                async move {
                    if failure == 1 {
                        StatusCode::SERVICE_UNAVAILABLE.into_response()
                    } else {
                        Json(if failure == 2 {
                            crate::common::id(999)
                        } else {
                            id
                        })
                        .into_response()
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    Server {
        job,
        report,
        mode,
        task,
        remote: Remote::new(
            &format!("http://{address}"),
            &"a".repeat(64).try_into().expect("token"),
        )
        .expect("remote"),
    }
}
async fn runtime(f: &Fixture, exe: PathBuf) -> NodeRuntime<LocalLauncher> {
    NodeRuntime::open(f.temp.path(), f.journal.identity(), LocalLauncher::new(exe))
        .await
        .expect("runtime")
}
async fn until(mut predicate: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !predicate().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("deadline");
}
#[tokio::test]
async fn public_recovery_keeps_work_across_restart_and_fences_duplicate_processes() {
    let (f, config, job) = saved_work().await;
    let _cleanup = Cleanup(f.temp.path().to_path_buf());
    let server = server(job.clone()).await;
    let mut node = runtime(&f, PathBuf::from("/missing-worker-for-recovery-test")).await;
    server.job.lock().expect("job").command.node_id = id(999);
    assert!(server.remote.dispatch(&mut node).await.is_err());
    *server.job.lock().expect("job") = job;
    server.mode.store(1, Ordering::SeqCst);
    assert!(server.remote.dispatch(&mut node).await.is_err());
    server.mode.store(0, Ordering::SeqCst);
    server
        .remote
        .dispatch(&mut node)
        .await
        .expect("retry inspection");
    let mut ready = server
        .report
        .lock()
        .expect("report")
        .clone()
        .expect("ready");
    assert_eq!(ready.state, RecoveryState::Ready);
    ready.state = RecoveryState::Approved;
    server.job.lock().expect("job").view = ready;
    assert!(
        server.remote.dispatch(&mut node).await.is_err(),
        "failed spawn keeps launch intent"
    );
    drop(node);
    let root = f.temp.path().canonicalize().expect("root");
    let first = root.join("first-worker");
    let second = root.join("second-worker");
    for (path, gate) in [(&first, "first"), (&second, "second")] {
        script(
            path,
            &format!(
                "echo $$ > '{}'\nwhile [ ! -f '{}' ]; do [ -d '{}' ] || exit 1; sleep 0.05; done\nexec '{}' \"$@\"",
                root.join(format!("{gate}.pid")).display(),
                root.join(gate).display(),
                root.display(),
                BINARY
            ),
        );
    }
    let mut node = runtime(&f, first).await;
    server.mode.store(2, Ordering::SeqCst);
    assert!(server.remote.dispatch(&mut node).await.is_err());
    server.mode.store(0, Ordering::SeqCst);
    server
        .remote
        .dispatch(&mut node)
        .await
        .expect("same instance retry");
    drop(node);
    let mut replacement = runtime(&f, second).await;
    server
        .remote
        .dispatch(&mut replacement)
        .await
        .expect("lost registration restart");
    std::fs::write(root.join("first"), "").expect("release original launch");
    until(async || config.directory.join("recovering.txt").is_file()).await;
    assert_eq!(f.session().await.state, SessionStatus::Running);
    std::fs::write(root.join("second"), "").expect("release competing launch");
    // The second child loses registration. Reaping it must not interrupt the first worker.
    wait_for_reap(&root, &mut replacement).await;
    assert_eq!(f.session().await.state, SessionStatus::Running);
    std::fs::write(root.join("continue"), "").expect("finish provider");
    until(async || f.status(6).await == CommandStatus::Completed).await;
    assert_eq!(
        std::fs::read_to_string(config.directory.join("completed.txt")).expect("existing work"),
        "original work"
    );
    assert_eq!(
        std::fs::read_to_string(config.directory.join("invocations.txt")).expect("calls"),
        "run\nrun\n"
    );
    assert!(config.directory.join("remaining.txt").is_file());
    assert_eq!(
        f.journal.pending_events().await.expect("one result").len(),
        1
    );
    server
        .remote
        .dispatch(&mut replacement)
        .await
        .expect("retry completed recovery");
    f.journal.request_stop_all().await.expect("cleanup");
    until(async || f.session().await.state == SessionStatus::Stopped).await;
}

async fn wait_for_reap(root: &Path, replacement: &mut NodeRuntime<LocalLauncher>) {
    until(async || root.join("second.pid").is_file()).await;
    let pid: u32 = std::fs::read_to_string(root.join("second.pid"))
        .expect("worker pid")
        .trim()
        .parse()
        .expect("numeric pid");
    until(async || {
        replacement.tick().await.expect("reap");
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("check test process")
            .success()
    })
    .await;
}
