#![allow(clippy::expect_used)]
use super::*;
use crate::{
    execution::ExecutionConfig,
    test_support::{Fixture, id, instance, start},
};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    permission::ProviderBinding,
};
use sqlx::Connection;
async fn setup() -> (Fixture, ExecutionConfig, RecoveryJob) {
    let f = Fixture::new().await;
    let root = f.temp.path().canonicalize().expect("root");
    let directory = root.join("work");
    std::fs::create_dir(&directory).expect("work");
    f.journal.mount(id(5), &directory).await.expect("mount");
    let instructions = root.join("prompts/execution/specialist.md");
    std::fs::create_dir_all(instructions.parent().expect("parent")).expect("prompts");
    std::fs::write(&instructions, "Test specialist").expect("prompt");
    let config = ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Saved work".into(),
            provider: LocalProvider::Claude,
            allow_edits: true,
        },
        executable: std::env::current_exe().expect("exe"),
        directory,
        instructions,
    };
    f.journal
        .configure_execution(&config)
        .await
        .expect("config");
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    f.journal
        .bind_provider(
            id(6),
            &instance().to_string(),
            &ProviderBinding {
                group_id: id(1),
                node_id: id(2),
                target: start().target,
                provider: orchestrator_protocol::capabilities::Capability::local_provider(
                    LocalProvider::Claude,
                ),
                provider_session_id: "saved-session".to_owned().try_into().expect("session"),
                resumable: true,
            },
        )
        .await
        .expect("binding");
    f.journal
        .provider_quiescence(&start(), &instance().to_string(), true)
        .await
        .expect("stopped");
    f.journal
        .interrupted(id(4), &instance().to_string(), "provider stopped")
        .await
        .expect("interrupt");
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
#[tokio::test]
async fn review_and_continuation_survive_restart_without_replaying_or_replacing_work() {
    let (f, config, mut job) = setup().await;
    let file = config.directory.join("already-done.txt");
    std::fs::write(&file, "keep me").expect("saved work");
    let (view, launch) = f.journal.recovery_plan(&job).await.expect("inspect");
    assert!(launch.is_none());
    assert_eq!(view.state, RecoveryState::Ready);
    assert_eq!(view.checkpoint.as_ref().expect("checkpoint").files, 1);
    assert_eq!(f.journal.recovery_plan(&job).await.expect("retry").0, view);
    job.view = view;
    job.view.state = RecoveryState::Approved;
    let restarted = Journal::attach(f.journal.path()).await.expect("restart");
    let (view, launch) = restarted.recovery_plan(&job).await.expect("resume");
    let launch = launch.expect("one launch identity");
    assert_eq!(view.state, RecoveryState::Resumed);
    assert_eq!(launch.command, start());
    let retry = restarted
        .recovery_plan(&job)
        .await
        .expect("lost response retry")
        .1
        .expect("unregistered launch");
    assert_eq!(retry.instance_id, launch.instance_id);
    assert!(
        restarted
            .heartbeat(id(4), &instance().to_string())
            .await
            .is_err()
    );
    restarted
        .register(id(4), &launch.instance_id, 1234)
        .await
        .expect("new worker");
    assert!(
        restarted
            .recovery_plan(&job)
            .await
            .expect("registered retry")
            .1
            .is_none()
    );
    restarted
        .claim(id(4), &launch.instance_id)
        .await
        .expect("claim")
        .expect("original turn continued");
    let message = restarted
        .recovery_message(&start(), "make the app", &config.instructions)
        .await
        .expect("message")
        .expect("recovery prompt");
    assert!(message.contains("not a new task"));
    assert!(message.contains("make the app"));
    assert_eq!(
        std::fs::read_to_string(file).expect("saved file"),
        "keep me"
    );
    assert_eq!(
        restarted
            .provider_binding(id(4))
            .await
            .expect("binding")
            .expect("session")
            .provider_session_id
            .as_str(),
        "saved-session"
    );
    restarted.close().await;
}
#[tokio::test]
async fn changed_files_invalidate_approval_and_reinspection_preserves_the_edits() {
    let (f, c, mut job) = setup().await;
    let file = c.directory.join("work.txt");
    std::fs::write(&file, "original").expect("file");
    job.view = f.journal.recovery_plan(&job).await.expect("review").0;
    job.view.state = RecoveryState::Approved;
    std::fs::write(&file, "modified").expect("edit");
    let (view, launch) = f.journal.recovery_plan(&job).await.expect("changed");
    assert_eq!(view.state, RecoveryState::Blocked);
    assert!(launch.is_none());
    assert_eq!(f.journal.recovery_plan(&job).await.expect("retry").0, view);
    job.view.id = id(101);
    job.view.state = RecoveryState::Inspecting;
    job.view.checkpoint = None;
    assert_eq!(
        f.journal
            .recovery_plan(&job)
            .await
            .expect("fresh review")
            .0
            .state,
        RecoveryState::Ready
    );
}
#[tokio::test]
async fn unsafe_or_missing_evidence_never_grants_a_launch() {
    for sql in [
        "DELETE FROM provider_quiescence",
        "UPDATE provider_quiescence SET stopped=0",
        "UPDATE local_sessions SET stop_requested=1",
        "UPDATE local_sessions SET state='running'",
        "UPDATE command_receipts SET status='queued'",
        "DELETE FROM provider_bindings",
        "DELETE FROM workspace_providers",
    ] {
        let (f, _, job) = setup().await;
        sqlx::query(sql)
            .execute(&f.journal.pool)
            .await
            .expect("fault");
        let (v, l) = f.journal.recovery_plan(&job).await.expect("blocked review");
        assert_eq!(v.state, RecoveryState::Blocked, "{sql}");
        assert!(l.is_none());
    }
}
#[tokio::test]
async fn malformed_requests_and_storage_failure_do_not_resume() {
    let (f, _, job) = setup().await;
    let mut bad = job.clone();
    bad.command.node_id = id(88);
    assert!(f.journal.recovery_plan(&bad).await.is_err());
    bad = job.clone();
    bad.view.command_id = id(88);
    assert!(f.journal.recovery_plan(&bad).await.is_err());
    bad = job.clone();
    bad.view.state = RecoveryState::Approved;
    assert!(f.journal.recovery_plan(&bad).await.is_err());
    let view = f.journal.recovery_plan(&job).await.expect("review").0;
    bad.view.checkpoint = view.checkpoint.clone();
    bad.view.checkpoint.as_mut().expect("checkpoint").digest = "changed".into();
    assert!(f.journal.recovery_plan(&bad).await.is_err());
    f.journal.close().await;
    assert!(f.journal.recovery_plan(&job).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn real_provider_failure_resumes_the_saved_session_and_keeps_completed_files() {
    use orchestrator_agents::Agent;
    use std::os::unix::fs::PermissionsExt;
    let (f, mut config, mut job) = setup().await;
    config.executable = f.temp.path().join("provider.sh");
    let body = r#"#!/bin/sh
input=$(cat)
if [ ! -f completed.txt ]; then
  printf 'kept original work' > completed.txt
  exit 1
fi
case " $* " in *" --resume "*) ;; *) exit 2;; esac
case "$input" in *"not a new task"*) ;; *) exit 3;; esac
[ "$(cat completed.txt)" = 'kept original work' ] || exit 4
printf 'finished remaining work' > remaining.txt
printf '%s\n' '{"type":"result","session_id":"saved-session","is_error":false,"subtype":"success","result":"Recovered without replacing completed work"}'
"#;
    std::fs::write(&config.executable, body).expect("provider");
    std::fs::set_permissions(&config.executable, std::fs::Permissions::from_mode(0o700))
        .expect("mode");
    sqlx::query("UPDATE workspace_providers SET configuration=?")
        .bind(Json(&config))
        .execute(&f.journal.pool)
        .await
        .expect("test provider config");
    sqlx::query("UPDATE local_sessions SET state='running'")
        .execute(&f.journal.pool)
        .await
        .expect("initial worker");
    sqlx::query("UPDATE command_receipts SET status='executing'")
        .execute(&f.journal.pool)
        .await
        .expect("initial claim");
    let mut agent = crate::local_provider::LocalAgent::new(
        f.journal.clone(),
        instance().to_string(),
        config.clone(),
        std::env::current_exe().expect("exe"),
    );
    assert!(agent.execute(&start()).await.is_err());
    f.journal
        .interrupted(id(4), &instance().to_string(), "provider disconnected")
        .await
        .expect("interrupted");
    assert_eq!(
        std::fs::read_to_string(config.directory.join("completed.txt")).expect("partial work"),
        "kept original work"
    );
    job.view = f.journal.recovery_plan(&job).await.expect("inspect").0;
    assert_eq!(job.view.state, RecoveryState::Ready);
    job.view.state = RecoveryState::Approved;
    let restarted = Journal::attach(f.journal.path())
        .await
        .expect("supervisor restart");
    let (_, launch) = restarted.recovery_plan(&job).await.expect("approve");
    let launch = launch.expect("permit");
    restarted
        .register(id(4), &launch.instance_id, 2000)
        .await
        .expect("register");
    restarted
        .claim(id(4), &launch.instance_id)
        .await
        .expect("claim");
    let mut agent = crate::local_provider::LocalAgent::new(
        restarted.clone(),
        launch.instance_id.clone(),
        config.clone(),
        std::env::current_exe().expect("exe"),
    );
    let event = agent.execute(&start()).await.expect("resumed provider");
    restarted
        .complete(
            id(6),
            &launch.instance_id,
            &crate::model::CommandOutcome::Completed {
                event: Box::new(event),
            },
        )
        .await
        .expect("completed");
    assert_eq!(
        std::fs::read_to_string(config.directory.join("completed.txt")).expect("kept"),
        "kept original work"
    );
    assert!(config.directory.join("remaining.txt").is_file());
    assert_eq!(restarted.pending_events().await.expect("events").len(), 1);
    restarted.close().await;
}

#[derive(Debug, Clone, Default)]
struct LauncherTest(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[derive(Debug)]
struct ProcessTest;
impl crate::process::ManagedProcess for ProcessTest {
    fn try_wait(&mut self) -> std::io::Result<Option<bool>> {
        Ok(None)
    }
}
impl crate::process::Launcher for LauncherTest {
    type Process = ProcessTest;
    fn spawn(&mut self, _: &Path, _: &LaunchSpec) -> std::io::Result<Self::Process> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ProcessTest)
    }
}
#[tokio::test]
async fn outbound_recovery_retries_lost_acknowledgments_without_duplicate_launch() {
    use axum::{Json, Router, routing::post};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    let (f, _, job) = setup().await;
    let current = Arc::new(Mutex::new(job));
    let report = Arc::new(Mutex::new(None));
    let mismatch = Arc::new(AtomicBool::new(false));
    let job_state = current.clone();
    let saved_report = report.clone();
    let bad_ack = mismatch.clone();
    let app = Router::new()
        .route(
            "/v1/dispatch/poll",
            post(
                move |Json(poll): Json<orchestrator_protocol::routing::DispatchPoll>| {
                    let job = job_state.lock().expect("job").clone();
                    async move {
                        Json(orchestrator_protocol::routing::DispatchBatch {
                            acknowledged: poll.accepted,
                            command: None,
                            recovery: Some(job),
                        })
                    }
                },
            ),
        )
        .route(
            "/v1/recovery/report",
            post(move |Json(view): Json<RecoveryView>| {
                let id = if bad_ack.load(Ordering::SeqCst) {
                    id(999)
                } else {
                    view.id
                };
                *saved_report.lock().expect("report") = Some(view);
                async move { Json(id) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
    let remote = crate::sync::Remote::new(
        &format!("http://{address}"),
        &"a".repeat(64).try_into().expect("token"),
    )
    .expect("remote");
    let launcher = LauncherTest::default();
    let mut runtime = NodeRuntime::open(f.temp.path(), f.journal.identity(), launcher.clone())
        .await
        .expect("runtime");
    remote.dispatch(&mut runtime).await.expect("inspect");
    let mut ready = report.lock().expect("report").clone().expect("ready");
    assert_eq!(ready.state, RecoveryState::Ready);
    ready.state = RecoveryState::Approved;
    current.lock().expect("job").view = ready;
    mismatch.store(true, Ordering::SeqCst);
    assert!(remote.dispatch(&mut runtime).await.is_err());
    assert_eq!(launcher.0.load(Ordering::SeqCst), 1);
    mismatch.store(false, Ordering::SeqCst);
    remote.dispatch(&mut runtime).await.expect("retry");
    assert_eq!(launcher.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.lock().expect("report").as_ref().expect("view").state,
        RecoveryState::Resumed
    );
    drop(runtime);
    let mut reopened = NodeRuntime::open(f.temp.path(), f.journal.identity(), launcher.clone())
        .await
        .expect("restart");
    sqlx::query("UPDATE local_sessions SET state='interrupted',pid=NULL")
        .execute(&f.journal.pool)
        .await
        .expect("unregistered launch expired");
    sqlx::query("UPDATE command_receipts SET status='uncertain'")
        .execute(&f.journal.pool)
        .await
        .expect("expire receipt");
    remote
        .dispatch(&mut reopened)
        .await
        .expect("recover durable launch after restart");
    assert_eq!(launcher.0.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn checkpoint_requires_matching_receipt_binding_and_accessible_workspace() {
    let (f, config, job) = setup().await;
    let mut command = job.command.clone();
    command.group_id = id(999);
    assert!(f.journal.checkpoint(&command).await.is_err());
    command = job.command.clone();
    command.command_id = id(999);
    assert!(f.journal.checkpoint(&command).await.is_err());
    command = job.command.clone();
    command.target.execution_epoch = command.target.execution_epoch.next().expect("epoch");
    assert!(f.journal.checkpoint(&command).await.is_err());
    assert!(
        f.journal
            .provider_quiescence(&job.command, "stale", true)
            .await
            .is_err()
    );
    std::fs::remove_dir(&config.directory).expect("remove workspace");
    let (view, launch) = f.journal.recovery_plan(&job).await.expect("blocked");
    assert_eq!(view.state, RecoveryState::Blocked);
    assert!(
        view.detail
            .expect("reason")
            .contains("could not be inspected")
    );
    assert!(launch.is_none());
    f.journal.close().await;
    assert!(
        f.journal
            .provider_quiescence(&job.command, "stale", true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn corrupt_or_missing_recovery_records_never_create_a_fresh_session() {
    let (f, config, mut job) = setup().await;
    job.view = f.journal.recovery_plan(&job).await.expect("review").0;
    job.view.state = RecoveryState::Approved;
    let mut invalid = job.view.clone();
    invalid.state = RecoveryState::Inspecting;
    sqlx::query("UPDATE task_recovery SET view=?")
        .bind(Json(&invalid))
        .execute(&f.journal.pool)
        .await
        .expect("corrupt view");
    assert!(f.journal.recovery_plan(&job).await.is_err());
    invalid.state = RecoveryState::Ready;
    invalid.checkpoint = None;
    job.view.checkpoint = None;
    sqlx::query("UPDATE task_recovery SET view=?")
        .bind(Json(&invalid))
        .execute(&f.journal.pool)
        .await
        .expect("missing checkpoint");
    assert!(f.journal.recovery_plan(&job).await.is_err());
    assert!(
        f.journal
            .retry_launch(&job.command, Some("other"))
            .await
            .expect("different worker")
            .is_none()
    );
    sqlx::query("UPDATE task_recovery SET instance='replacement'")
        .execute(&f.journal.pool)
        .await
        .expect("saved intent");
    let binding = f
        .journal
        .provider_binding(id(4))
        .await
        .expect("binding")
        .expect("saved");
    let mut changed = binding.clone();
    changed.target.execution_epoch = changed.target.execution_epoch.next().expect("epoch");
    sqlx::query("UPDATE provider_bindings SET binding=?")
        .bind(Json(&changed))
        .execute(&f.journal.pool)
        .await
        .expect("changed binding");
    assert!(
        f.journal
            .recovery_message(&job.command, "original", &config.instructions)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM provider_bindings")
        .execute(&f.journal.pool)
        .await
        .expect("lost binding");
    assert!(
        f.journal
            .recovery_message(&job.command, "original", &config.instructions)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM workspace_providers")
        .execute(&f.journal.pool)
        .await
        .expect("lost config");
    assert!(
        f.journal
            .recovery_launch(&job.command, "new".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn editable_recovery_instructions_are_loaded_and_invalid_edits_fail_closed() {
    let (f, config, mut job) = setup().await;
    job.view = f.journal.recovery_plan(&job).await.expect("review").0;
    job.view.state = RecoveryState::Approved;
    f.journal.recovery_plan(&job).await.expect("continue");
    let prompt = config.instructions.with_file_name("recovery.md");
    std::fs::write(&prompt, "Inspect the existing result before continuing.")
        .expect("editable prompt");
    assert!(
        f.journal
            .recovery_message(&job.command, "original", &config.instructions)
            .await
            .expect("prompt")
            .expect("recovery")
            .starts_with("Inspect the existing result")
    );
    std::fs::write(&prompt, "").expect("invalid prompt");
    assert!(
        f.journal
            .recovery_message(&job.command, "original", &config.instructions)
            .await
            .is_err()
    );
}

#[test]
fn workspace_inspection_is_bounded_and_does_not_follow_symlinks() {
    let temp = tempfile::tempdir().expect("workspace");
    let root = temp.path().canonicalize().expect("root");
    std::fs::create_dir(root.join("src")).expect("directory");
    std::fs::write(root.join("src/done.txt"), "saved").expect("file");
    std::fs::create_dir(root.join("node_modules")).expect("ignored");
    std::fs::write(root.join("node_modules/ignored.txt"), "dependency").expect("ignored file");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/", root.join("outside")).expect("symlink");
    assert_eq!(
        inspect_files(
            &root,
            &root,
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .expect("inspect"),
        1
    );
    assert!(
        inspect_files(
            &root,
            &root,
            17,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(
        inspect_files(
            &root,
            &root,
            0,
            &mut Sha256::new(),
            &mut ScanBudget {
                entries: 10000,
                ..ScanBudget::default()
            }
        )
        .is_err()
    );
    assert!(
        inspect_files(
            &root.join("src"),
            &root,
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(
        inspect_files(
            &root,
            &root.join("missing"),
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(
        inspect_files(
            &root,
            &root.join("src/done.txt"),
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
}

#[tokio::test]
async fn continuation_write_failures_roll_back_and_allow_the_same_approval_to_retry() {
    for fault in [
        "CREATE TRIGGER fail BEFORE UPDATE ON local_sessions BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON local_sessions BEGIN SELECT RAISE(IGNORE); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON command_receipts BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON task_recovery BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail AFTER UPDATE ON task_recovery BEGIN INSERT INTO child VALUES(42); END",
    ] {
        let (f, _, mut job) = setup().await;
        job.view = f.journal.recovery_plan(&job).await.expect("review").0;
        job.view.state = RecoveryState::Approved;
        sqlx::raw_sql(fault)
            .execute(&f.journal.pool)
            .await
            .expect("fault");
        assert!(f.journal.recovery_plan(&job).await.is_err(), "{fault}");
        assert_eq!(
            f.journal
                .command(id(6))
                .await
                .expect("receipt")
                .expect("saved")
                .status,
            CommandStatus::Uncertain
        );
        sqlx::query("DROP TRIGGER fail")
            .execute(&f.journal.pool)
            .await
            .expect("restore");
        assert!(
            f.journal
                .recovery_plan(&job)
                .await
                .expect("retry")
                .1
                .is_some()
        );
    }
}

#[tokio::test]
async fn missing_launch_registration_can_retry_but_a_stop_request_cannot() {
    for fault in [
        "UPDATE local_sessions SET stop_requested=1",
        "CREATE TRIGGER fail BEFORE UPDATE ON local_sessions BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TRIGGER fail BEFORE UPDATE ON command_receipts BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail AFTER UPDATE ON command_receipts BEGIN INSERT INTO child VALUES(42); END",
    ] {
        let (f, _, mut job) = setup().await;
        job.view = f.journal.recovery_plan(&job).await.expect("review").0;
        job.view.state = RecoveryState::Approved;
        f.journal
            .recovery_plan(&job)
            .await
            .expect("save launch intent");
        sqlx::raw_sql("UPDATE local_sessions SET state='interrupted',pid=NULL; UPDATE command_receipts SET status='uncertain'").execute(&f.journal.pool).await.expect("expired launch");
        sqlx::raw_sql(fault)
            .execute(&f.journal.pool)
            .await
            .expect("fault");
        assert!(f.journal.recovery_plan(&job).await.is_err(), "{fault}");
    }
}

#[tokio::test]
async fn inspection_and_launch_storage_errors_are_explicit() {
    for table in [
        "command_receipts",
        "local_sessions",
        "provider_quiescence",
        "runtime_permissions",
        "provider_tools",
        "provider_bindings",
        "workspace_providers",
        "execution_activity",
    ] {
        let (f, config, job) = setup().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} RENAME TO hidden"
        )))
        .execute(&f.journal.pool)
        .await
        .expect("fault");
        assert!(f.journal.checkpoint(&job.command).await.is_err(), "{table}");
        if table == "local_sessions" {
            assert!(f.journal.retry_launch(&job.command, None).await.is_err());
        }
        if table == "workspace_providers" {
            assert!(
                f.journal
                    .recovery_launch(&job.command, "instance".into())
                    .await
                    .is_err()
            );
        }
        if table == "provider_bindings" {
            sqlx::query("INSERT INTO task_recovery(request_id,command_id,view,instance) VALUES(?,?,?,'saved')").bind(job.view.id.to_string()).bind(job.command.command_id.to_string()).bind(Json(&job.view)).execute(&f.journal.pool).await.expect("intent");
            assert!(
                f.journal
                    .recovery_message(&job.command, "original", &config.instructions)
                    .await
                    .is_err()
            );
        }
    }
    let (f, config, job) = setup().await;
    sqlx::query("ALTER TABLE task_recovery RENAME TO hidden")
        .execute(&f.journal.pool)
        .await
        .expect("fault");
    assert!(f.journal.inspect_recovery(&job).await.is_err());
    assert!(
        f.journal
            .recovery_message(&job.command, "original", &config.instructions)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn foreign_provider_and_missing_session_are_not_resumable() {
    let (f, _, job) = setup().await;
    sqlx::query(
        "UPDATE provider_bindings SET binding=json_set(binding,'$.provider','provider.codex')",
    )
    .execute(&f.journal.pool)
    .await
    .expect("wrong provider");
    assert!(f.journal.checkpoint(&job.command).await.is_err());
    let mut connection = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(f.journal.path())
            .foreign_keys(false),
    )
    .await
    .expect("fault connection");
    sqlx::query("DELETE FROM local_sessions")
        .execute(&mut connection)
        .await
        .expect("lost session");
    assert!(f.journal.checkpoint(&job.command).await.is_err());
    assert!(f.journal.retry_launch(&job.command, None).await.is_err());
}

#[tokio::test]
async fn pending_approval_or_unknown_tool_result_blocks_continuation() {
    for fault in [
        "INSERT INTO runtime_permissions(permission_id,command_id,record) SELECT 'review',command_id,'{\"state\":\"waiting\"}' FROM command_receipts",
        "INSERT INTO provider_tools(call_id,parent,request) SELECT 'tool',command_id,'{}' FROM command_receipts",
    ] {
        let (f, _, job) = setup().await;
        sqlx::query(fault)
            .execute(&f.journal.pool)
            .await
            .expect("pending action");
        let view = f.journal.recovery_plan(&job).await.expect("inspect").0;
        assert_eq!(view.state, RecoveryState::Blocked);
        assert!(view.detail.expect("reason").contains("unresolved"));
    }
}

#[tokio::test]
async fn changed_checkpoint_write_failure_never_launches() {
    let (f, config, mut job) = setup().await;
    job.view = f.journal.recovery_plan(&job).await.expect("inspect").0;
    job.view.state = RecoveryState::Approved;
    std::fs::write(config.directory.join("new.txt"), "edit").expect("new file");
    sqlx::query("CREATE TRIGGER fail BEFORE UPDATE ON task_recovery BEGIN SELECT RAISE(ABORT,'write failed'); END").execute(&f.journal.pool).await.expect("fault");
    assert!(f.journal.recovery_plan(&job).await.is_err());
}

#[derive(Debug, Clone)]
struct ExitLauncher {
    pid: u32,
    fail: bool,
    transient: bool,
}
#[derive(Debug)]
struct ExitProcess(u32, u8);
impl crate::process::ManagedProcess for ExitProcess {
    fn pid(&self) -> Option<u32> {
        Some(self.0)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<bool>> {
        match self.1 {
            2 => {
                self.1 = 1;
                Err(std::io::Error::other("temporary wait failure"))
            }
            1 => {
                self.1 = 0;
                Ok(None)
            }
            _ => Ok(Some(false)),
        }
    }
}
impl Launcher for ExitLauncher {
    type Process = ExitProcess;
    fn spawn(&mut self, _: &Path, _: &LaunchSpec) -> std::io::Result<ExitProcess> {
        if self.fail {
            Err(std::io::Error::other("launch failed"))
        } else {
            Ok(ExitProcess(self.pid, if self.transient { 2 } else { 0 }))
        }
    }
}
#[tokio::test]
async fn failed_or_duplicate_recovery_launch_cannot_interrupt_the_registered_owner() {
    let (f, _, mut job) = setup().await;
    job.view = f.journal.recovery_plan(&job).await.expect("inspect").0;
    job.view.state = RecoveryState::Approved;
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        ExitLauncher {
            pid: 100,
            fail: true,
            transient: false,
        },
    )
    .await
    .expect("runtime");
    assert!(runtime.recover(&job).await.is_err());
    drop(runtime);
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        ExitLauncher {
            pid: 100,
            fail: false,
            transient: false,
        },
    )
    .await
    .expect("restart");
    runtime.recover(&job).await.expect("retry launch");
    let session = f
        .journal
        .session(id(4))
        .await
        .expect("session")
        .expect("saved");
    f.journal
        .register(id(4), &session.instance_id, 200)
        .await
        .expect("other launch wins registration");
    runtime.tick().await.expect("reap loser");
    assert_eq!(
        f.journal
            .session(id(4))
            .await
            .expect("session")
            .expect("saved")
            .state,
        SessionStatus::Running
    );
}
#[tokio::test]
async fn reaping_checks_storage_before_forgetting_the_child() {
    let (f, _, mut job) = setup().await;
    job.view = f.journal.recovery_plan(&job).await.expect("inspect").0;
    job.view.state = RecoveryState::Approved;
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        ExitLauncher {
            pid: 100,
            fail: false,
            transient: false,
        },
    )
    .await
    .expect("runtime");
    runtime.recover(&job).await.expect("launch");
    sqlx::query("ALTER TABLE local_sessions RENAME TO hidden")
        .execute(&f.journal.pool)
        .await
        .expect("fault");
    assert!(runtime.tick().await.is_err());
    sqlx::query("ALTER TABLE hidden RENAME TO local_sessions")
        .execute(&f.journal.pool)
        .await
        .expect("restore");
    runtime.tick().await.expect("reap unregistered child");
}
#[test]
fn real_child_exposes_its_process_identity() {
    use crate::process::ManagedProcess;
    let mut child = std::process::Command::new(std::env::current_exe().expect("exe"))
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("child");
    assert_eq!(ManagedProcess::pid(&child), Some(child.id()));
    child.wait().expect("reap");
}

#[test]
fn interrupted_directory_reads_and_file_reads_are_not_accepted_as_checkpoints() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().canonicalize().expect("root");
    let file = root.join("work");
    std::fs::write(&file, "saved").expect("file");
    assert!(
        inspect_entries(
            &root,
            &mut std::iter::once(Err(std::io::Error::other("directory read interrupted"))),
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(
        inspect_entries(
            &root,
            &mut std::iter::once(Ok(root.join("deleted"))),
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(
        inspect_entries(
            &root.join("other"),
            &mut std::iter::once(Ok(file)),
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .is_err()
    );
    assert!(hash_file(&root.join("deleted"), &mut Sha256::new(), &mut 10).is_err());
    assert!(hash_contents("oversized".as_bytes(), &mut Sha256::new(), &mut 2).is_err());
    assert!(hash_contents(Broken, &mut Sha256::new(), &mut 10).is_err());
    let mut budget = ScanBudget::default();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).expect("dir");
    assert!(
        inspect_entries(
            &root,
            &mut std::iter::once(Ok(nested)),
            16,
            &mut Sha256::new(),
            &mut budget
        )
        .is_err()
    );
    let mut budget = ScanBudget {
        bytes: 0,
        ..ScanBudget::default()
    };
    assert!(inspect_files(&root, &root, 0, &mut Sha256::new(), &mut budget).is_err());
}

struct Broken;
impl std::io::Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("file read interrupted"))
    }
}

#[tokio::test]
async fn another_uncertain_local_command_blocks_recovery_instead_of_stalling_after_launch() {
    let (f, _, job) = setup().await;
    sqlx::query("UPDATE local_sessions SET state='running'")
        .execute(&f.journal.pool)
        .await
        .expect("live worker");
    f.journal
        .accept(&crate::test_support::send(77), uuid::Uuid::new_v4())
        .await
        .expect("queued followup");
    f.journal
        .interrupted(id(4), &instance().to_string(), "stopped")
        .await
        .expect("interrupted");
    let (view, launch) = f.journal.recovery_plan(&job).await.expect("review");
    assert_eq!(view.state, RecoveryState::Blocked);
    assert!(launch.is_none());
}

#[tokio::test]
async fn unsupported_or_oversized_workspaces_and_failed_review_writes_never_start() {
    let (f, config, job) = setup().await;
    let mut path = config.directory.clone();
    for _ in 0..17 {
        path = path.join("nested");
    }
    std::fs::create_dir_all(path).expect("deep workspace");
    assert_eq!(
        f.journal
            .recovery_plan(&job)
            .await
            .expect("blocked")
            .0
            .state,
        RecoveryState::Blocked
    );
    let (f, _, job) = setup().await;
    sqlx::query("CREATE TRIGGER fail BEFORE INSERT ON task_recovery BEGIN SELECT RAISE(ABORT,'review unavailable'); END").execute(&f.journal.pool).await.expect("fault");
    assert!(f.journal.recovery_plan(&job).await.is_err());
}
#[cfg(unix)]
#[test]
fn special_files_are_not_read_as_project_files() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().canonicalize().expect("root");
    let _socket = std::os::unix::net::UnixListener::bind(root.join("socket")).expect("socket");
    assert_eq!(
        inspect_files(
            &root,
            &root,
            0,
            &mut Sha256::new(),
            &mut ScanBudget::default()
        )
        .expect("inspect"),
        0
    );
}

async fn fault_after_reads(journal: &Journal, after: usize, statement: &'static str) -> Journal {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .after_release(move |connection, _| {
            let inject = count.fetch_add(1, Ordering::SeqCst) + 1 == after;
            Box::pin(async move {
                if inject {
                    sqlx::raw_sql(statement).execute(connection).await?;
                }
                Ok(true)
            })
        })
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(journal.path())
                .foreign_keys(true),
        )
        .await
        .expect("fault pool");
    let mut copy = journal.clone();
    copy.pool = pool;
    copy
}
#[tokio::test]
async fn storage_changes_between_inspection_and_launch_do_not_authorize_new_work() {
    for (after, statement) in [
        (7, "DELETE FROM workspace_providers"),
        (8, "PRAGMA query_only=ON"),
    ] {
        let (f, _, mut job) = setup().await;
        job.view = f.journal.recovery_plan(&job).await.expect("review").0;
        job.view.state = RecoveryState::Approved;
        let faulty = fault_after_reads(&f.journal, after, statement).await;
        assert!(faulty.recovery_plan(&job).await.is_err(), "{after}");
        faulty.close().await;
        assert_eq!(
            f.journal
                .command(id(6))
                .await
                .expect("receipt")
                .expect("saved")
                .status,
            CommandStatus::Uncertain
        );
    }
}
#[tokio::test]
async fn pending_launch_storage_changes_fail_without_resetting_session_identity() {
    for (interrupted, statement) in [
        (true, "PRAGMA query_only=ON"),
        (false, "DELETE FROM workspace_providers"),
    ] {
        let (f, _, mut job) = setup().await;
        job.view = f.journal.recovery_plan(&job).await.expect("review").0;
        job.view.state = RecoveryState::Approved;
        let launch = f
            .journal
            .recovery_plan(&job)
            .await
            .expect("intent")
            .1
            .expect("launch");
        if interrupted {
            sqlx::query("UPDATE local_sessions SET state='interrupted'")
                .execute(&f.journal.pool)
                .await
                .expect("interruption");
        }
        let faulty = fault_after_reads(&f.journal, 1, statement).await;
        assert!(
            faulty
                .retry_launch(&job.command, Some(&launch.instance_id))
                .await
                .is_err()
        );
        faulty.close().await;
    }
}

#[tokio::test]
async fn malformed_jobs_and_failed_report_transport_preserve_the_saved_task() {
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{Arc, Mutex};
    let (f, _, job) = setup().await;
    let mut wrong = job.clone();
    wrong.command.node_id = id(999);
    let selected = Arc::new(Mutex::new(wrong));
    let state = selected.clone();
    let app = Router::new()
        .route(
            "/v1/dispatch/poll",
            post(
                move |Json(poll): Json<orchestrator_protocol::routing::DispatchPoll>| {
                    let recovery = state.lock().expect("state").clone();
                    async move {
                        Json(orchestrator_protocol::routing::DispatchBatch {
                            acknowledged: poll.accepted,
                            command: None,
                            recovery: Some(recovery),
                        })
                    }
                },
            ),
        )
        .route(
            "/v1/recovery/report",
            post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });
    let remote = crate::sync::Remote::new(
        &format!("http://{address}"),
        &"a".repeat(64).try_into().expect("token"),
    )
    .expect("remote");
    let launcher = LauncherTest::default();
    let mut runtime = NodeRuntime::open(f.temp.path(), f.journal.identity(), launcher.clone())
        .await
        .expect("runtime");
    assert!(remote.dispatch(&mut runtime).await.is_err());
    *selected.lock().expect("state") = job;
    assert!(remote.dispatch(&mut runtime).await.is_err());
    assert_eq!(launcher.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        f.journal
            .command(id(6))
            .await
            .expect("receipt")
            .expect("saved")
            .status,
        CommandStatus::Uncertain
    );
    server.abort();
}

#[tokio::test]
async fn recovery_reviews_and_repeated_launches_remain_idempotent_before_reaping() {
    let (f, _, mut job) = setup().await;
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        ExitLauncher {
            pid: 100,
            fail: false,
            transient: false,
        },
    )
    .await
    .expect("runtime");
    let mut bad = job.clone();
    bad.command.node_id = id(999);
    assert!(runtime.recover(&bad).await.is_err());
    job.view = runtime.recover(&job).await.expect("read-only review");
    assert_eq!(job.view.state, RecoveryState::Ready);
    job.view.state = RecoveryState::Approved;
    let first = runtime.recover(&job).await.expect("launch");
    assert_eq!(runtime.recover(&job).await.expect("same launch"), first);
}
#[test]
fn hashing_reads_exact_contents_and_surfaces_file_read_failures() {
    let mut digest = Sha256::new();
    let mut budget = 3;
    hash_contents("abc".as_bytes(), &mut digest, &mut budget).expect("hash contents");
    assert_eq!(digest.finalize(), Sha256::digest(b"abc"));
    assert_eq!(budget, 0);
    let temp = tempfile::tempdir().expect("directory");
    assert!(hash_file(temp.path(), &mut Sha256::new(), &mut 10).is_err());
}

#[tokio::test]
async fn local_launcher_failure_and_unregistered_exit_preserve_the_recovery_intent() {
    use crate::process::LocalLauncher;
    let (f, _, mut job) = setup().await;
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        LocalLauncher::new("/missing-recovery-worker".into()),
    )
    .await
    .expect("runtime");
    let mut bad = job.clone();
    bad.command.node_id = id(999);
    assert!(runtime.recover(&bad).await.is_err());
    job.view = runtime.recover(&job).await.expect("inspect");
    job.view.state = RecoveryState::Approved;
    assert!(runtime.recover(&job).await.is_err());
    drop(runtime);
    // This test executable cannot register as a worker: process creation succeeds,
    // but worker startup fails. The real worker path is covered by integration tests.
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        LocalLauncher::new(std::env::current_exe().expect("test executable")),
    )
    .await
    .expect("restart");
    let view = runtime.recover(&job).await.expect("retry launch");
    assert_eq!(runtime.recover(&job).await.expect("same launch"), view);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            runtime.tick().await.expect("reap unregistered process");
            if f.journal
                .session(id(4))
                .await
                .expect("session")
                .expect("saved")
                .state
                == SessionStatus::Interrupted
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exit deadline");
    assert_eq!(
        f.journal
            .command(id(6))
            .await
            .expect("receipt")
            .expect("saved")
            .status,
        CommandStatus::Uncertain
    );
}

#[tokio::test]
async fn recovery_reaping_retries_wait_and_journal_failures_without_forgetting_work() {
    let (f, _, mut job) = setup().await;
    let mut runtime = NodeRuntime::open(
        f.temp.path(),
        f.journal.identity(),
        ExitLauncher {
            pid: 100,
            fail: false,
            transient: true,
        },
    )
    .await
    .expect("runtime");
    job.view = runtime.recover(&job).await.expect("inspect");
    job.view.state = RecoveryState::Approved;
    runtime.recover(&job).await.expect("launch");
    assert!(runtime.tick().await.is_err());
    runtime.tick().await.expect("still running");
    sqlx::query("CREATE TRIGGER fail_interruption BEFORE UPDATE ON local_sessions BEGIN SELECT RAISE(ABORT,'temporary journal failure'); END")
        .execute(&f.journal.pool).await.expect("fault");
    assert!(runtime.tick().await.is_err());
    assert_eq!(
        f.journal
            .session(id(4))
            .await
            .expect("session")
            .expect("saved")
            .state,
        SessionStatus::Starting
    );
    sqlx::query("DROP TRIGGER fail_interruption")
        .execute(&f.journal.pool)
        .await
        .expect("restore");
    runtime.tick().await.expect("retry reap");
    assert_eq!(
        f.journal
            .session(id(4))
            .await
            .expect("session")
            .expect("saved")
            .state,
        SessionStatus::Interrupted
    );
    assert_eq!(
        f.journal
            .command(id(6))
            .await
            .expect("receipt")
            .expect("saved")
            .status,
        CommandStatus::Uncertain
    );
}
