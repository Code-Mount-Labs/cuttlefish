//! Runtime ownership, process failures, and worker tests against a real journal.
#![allow(clippy::expect_used)]

use crate::common;

use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use common::TestAgent as FakeAgent;
use common::{Fixture, cancel, id, identity, instance, send, start};
use orchestrator_agents::Agent;
use orchestrator_node::{
    NodeError,
    model::{CommandStatus, LaunchSpec, SessionStatus},
    process::{Launcher, ManagedProcess},
    runtime::NodeRuntime,
    worker,
};
use orchestrator_protocol::agent::{AgentCommand, AgentEvent};

#[derive(Debug, Clone, Default)]
struct TestLauncher {
    launches: Arc<AtomicUsize>,
    state: Arc<AtomicU8>,
}

#[derive(Debug)]
struct TestProcess(Arc<AtomicU8>);

impl ManagedProcess for TestProcess {
    fn try_wait(&mut self) -> io::Result<Option<bool>> {
        match self.0.load(Ordering::SeqCst) {
            1 => Ok(Some(true)),
            2 => Ok(Some(false)),
            3 => Err(io::Error::other("wait failed")),
            _ => Ok(None),
        }
    }
}

impl Launcher for TestLauncher {
    type Process = TestProcess;
    fn spawn(&mut self, _: &Path, _: &LaunchSpec) -> io::Result<TestProcess> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        if self.state.load(Ordering::SeqCst) == 4 {
            return Err(io::Error::other("spawn failed"));
        }
        Ok(TestProcess(Arc::clone(&self.state)))
    }
}

#[tokio::test]
async fn lock_prevents_two_supervisors_and_recovery_never_respawns() {
    let temp = tempfile::tempdir().expect("directory");
    let launcher = TestLauncher::default();
    let mut runtime = NodeRuntime::open(temp.path(), identity(), launcher.clone())
        .await
        .expect("runtime");
    let mut misrouted = start();
    misrouted.node_id = id(99);
    assert!(runtime.submit(&misrouted).await.is_err());
    assert!(
        NodeRuntime::open(temp.path(), identity(), launcher.clone())
            .await
            .is_err()
    );
    runtime
        .journal()
        .mount(id(5), temp.path())
        .await
        .expect("mount");
    runtime.submit(&start()).await.expect("start");
    runtime.submit(&start()).await.expect("repeat");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    runtime.tick().await.expect("live child");
    runtime.journal().close().await;
    drop(runtime);
    let mut replacement = NodeRuntime::open(temp.path(), identity(), launcher.clone())
        .await
        .expect("replacement");
    replacement
        .submit(&start())
        .await
        .expect("recover saved start");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    replacement.journal().close().await;
}

#[tokio::test]
async fn spawn_failure_is_durable_and_not_automatically_retried() {
    let temp = tempfile::tempdir().expect("directory");
    let launcher = TestLauncher::default();
    launcher.state.store(4, Ordering::SeqCst);
    let mut runtime = NodeRuntime::open(temp.path(), identity(), launcher.clone())
        .await
        .expect("runtime");
    runtime
        .journal()
        .mount(id(5), temp.path())
        .await
        .expect("mount");
    assert!(runtime.submit(&start()).await.is_err());
    assert_eq!(
        runtime
            .submit(&start())
            .await
            .expect("saved failure")
            .status,
        CommandStatus::Uncertain
    );
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    assert!(
        runtime
            .journal()
            .session(id(4))
            .await
            .expect("session")
            .expect("saved")
            .last_error
            .expect("reason")
            .contains("spawn failed")
    );
    runtime.journal().close().await;
}

#[tokio::test]
async fn process_exit_is_reaped_and_wait_errors_are_explicit() {
    for outcome in [1, 2, 3] {
        let temp = tempfile::tempdir().expect("directory");
        let launcher = TestLauncher::default();
        let mut runtime = NodeRuntime::open(temp.path(), identity(), launcher.clone())
            .await
            .expect("runtime");
        runtime
            .journal()
            .mount(id(5), temp.path())
            .await
            .expect("mount");
        runtime.submit(&start()).await.expect("start");
        launcher.state.store(outcome, Ordering::SeqCst);
        if outcome == 3 {
            assert!(runtime.tick().await.is_err());
            launcher.state.store(1, Ordering::SeqCst);
        }
        runtime.tick().await.expect("reap");
        runtime.tick().await.expect("already reaped");
        assert_eq!(
            runtime
                .journal()
                .session(id(4))
                .await
                .expect("session")
                .expect("saved")
                .state,
            SessionStatus::Interrupted
        );
        runtime.journal().close().await;
    }
}

#[tokio::test]
async fn runtime_startup_rejects_files_and_wrong_identities() {
    let fixture = Fixture::new().await;
    assert!(
        NodeRuntime::open(fixture.journal.path(), identity(), TestLauncher::default())
            .await
            .is_err()
    );
    let mut other = identity();
    other.group_id = id(77);
    assert!(
        NodeRuntime::open(fixture.temp.path(), other, TestLauncher::default())
            .await
            .is_err()
    );
    fixture.journal.close().await;
    let temp = tempfile::tempdir().expect("directory");
    std::fs::create_dir(temp.path().join("supervisor.lock")).expect("obstruct lock");
    assert!(
        NodeRuntime::open(temp.path(), identity(), TestLauncher::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn process_failure_reporting_propagates_storage_errors_and_tick_can_retry() {
    for state in [1, 4] {
        let fixture = Fixture::new().await;
        let launcher = TestLauncher::default();
        let mut runtime = NodeRuntime::open(fixture.temp.path(), identity(), launcher.clone())
            .await
            .expect("runtime");
        if state == 1 {
            runtime.submit(&start()).await.expect("launch");
        }
        let raw = fixture.raw().await;
        sqlx::query("CREATE TRIGGER fail_update BEFORE UPDATE ON local_sessions BEGIN SELECT RAISE(ABORT,'storage failed'); END")
            .execute(&raw).await.expect("failure trigger");
        launcher.state.store(state, Ordering::SeqCst);
        if state == 1 {
            assert!(matches!(runtime.tick().await, Err(NodeError::Database(_))));
        } else {
            assert!(matches!(
                runtime.submit(&start()).await,
                Err(NodeError::Database(_))
            ));
        }
        assert_eq!(fixture.session().await.state, SessionStatus::Starting);
        sqlx::query("DROP TRIGGER fail_update")
            .execute(&raw)
            .await
            .expect("repair storage");
        if state == 1 {
            runtime.tick().await.expect("retry reconciliation");
            assert_eq!(fixture.session().await.state, SessionStatus::Interrupted);
        }
        raw.close().await;
        runtime.journal().close().await;
        fixture.journal.close().await;
    }
}

#[tokio::test]
async fn crash_between_persisting_intent_and_spawning_does_not_replay_unknown_work() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    let launcher = TestLauncher::default();
    let mut runtime = NodeRuntime::open(fixture.temp.path(), identity(), launcher.clone())
        .await
        .expect("runtime");
    runtime.submit(&start()).await.expect("original receipt");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 0);
    runtime
        .journal()
        .expire(0)
        .await
        .expect("expire missing worker");
    assert_eq!(fixture.status(6).await, CommandStatus::Uncertain);
    runtime.journal().close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn local_process_is_observed_running_and_reaped_after_cooperative_stop() {
    use orchestrator_node::process::LocalLauncher;
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = Fixture::new().await;
    let spec = fixture
        .journal
        .accept(&start(), instance())
        .await
        .expect("accept")
        .launch
        .expect("launch permit");
    let mut launcher = LocalLauncher::new(env!("CARGO_BIN_EXE_orchestrator-node").into());
    let mut child = ChildGuard(
        launcher
            .spawn(fixture.journal.path(), &spec)
            .expect("launch"),
    );
    wait_for_command(&fixture.journal, 6, CommandStatus::Completed).await;
    assert_eq!(
        ManagedProcess::try_wait(&mut child.0).expect("running"),
        None
    );
    fixture.journal.request_stop_all().await.expect("stop");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(success) = ManagedProcess::try_wait(&mut child.0).expect("reap") {
                assert!(success);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded shutdown");
    assert_eq!(fixture.session().await.state, SessionStatus::Stopped);
    fixture.journal.close().await;
}

async fn wait_for_command(
    journal: &orchestrator_node::journal::Journal,
    number: u128,
    state: CommandStatus,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if journal
                .command(id(number))
                .await
                .expect("receipt")
                .expect("saved")
                .status
                == state
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bounded worker result");
}

#[tokio::test]
async fn worker_executes_persisted_commands_and_confirms_cancellation() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    let journal = fixture.journal.clone();
    let worker = tokio::spawn(async move {
        worker::run(
            &journal,
            id(4),
            &instance().to_string(),
            FakeAgent::new(id(1), id(2)),
        )
        .await
    });
    wait_for_command(&fixture.journal, 6, CommandStatus::Completed).await;
    fixture
        .journal
        .accept(&send(11), instance())
        .await
        .expect("send");
    wait_for_command(&fixture.journal, 11, CommandStatus::Completed).await;
    fixture
        .journal
        .accept(&cancel(12), instance())
        .await
        .expect("cancel");
    worker.await.expect("worker task").expect("worker shutdown");
    assert_eq!(
        fixture
            .journal
            .session(id(4))
            .await
            .expect("session")
            .expect("saved")
            .state,
        SessionStatus::Stopped
    );
    assert_eq!(
        fixture
            .journal
            .pending_events()
            .await
            .expect("events")
            .len(),
        3
    );
    fixture.journal.close().await;
}

#[tokio::test]
async fn adapter_failures_are_saved_and_operator_stop_is_cooperative() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    let journal = fixture.journal.clone();
    let mut agent = FakeAgent::new(id(1), id(2));
    agent.fail_next_command();
    let worker =
        tokio::spawn(
            async move { worker::run(&journal, id(4), &instance().to_string(), agent).await },
        );
    wait_for_command(&fixture.journal, 6, CommandStatus::Rejected).await;
    fixture
        .journal
        .request_stop_all()
        .await
        .expect("stop requested");
    worker.await.expect("worker task").expect("stopped");
    fixture.journal.close().await;
}

#[tokio::test]
async fn a_busy_specialist_keeps_heartbeating_and_stops_if_its_journal_is_lost() {
    for lose_storage in [false, true] {
        let fixture = Fixture::new().await;
        fixture.accepted().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut agent = FakeAgent::new(id(1), id(2));
        agent.hold = Some((Arc::clone(&entered), Arc::clone(&release)));
        let journal = fixture.journal.clone();
        let worker = tokio::spawn(async move {
            worker::run(&journal, id(4), &instance().to_string(), agent).await
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .expect("adapter started");
        let raw = fixture.raw().await;
        sqlx::query("UPDATE local_sessions SET heartbeat_at=0")
            .execute(&raw)
            .await
            .expect("reset heartbeat observation");
        raw.close().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.session().await.heartbeat_at == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("heartbeat while adapter is still held");
        fixture
            .journal
            .expire(30_000)
            .await
            .expect("liveness check");
        assert_eq!(fixture.session().await.state, SessionStatus::Running);
        assert_eq!(fixture.status(6).await, CommandStatus::Executing);
        if lose_storage {
            fixture.journal.close().await;
        } else {
            release.notify_one();
            wait_for_command(&fixture.journal, 6, CommandStatus::Completed).await;
            fixture
                .journal
                .request_stop_all()
                .await
                .expect("stop worker");
        }
        let result = tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .expect("bounded worker exit")
            .expect("worker task");
        assert_eq!(result.is_err(), lose_storage);
        fixture.journal.close().await;
    }
}

#[derive(Debug)]
struct WrongAdapter;

impl Agent for WrongAdapter {
    type Error = std::convert::Infallible;
    async fn execute(&mut self, command: &AgentCommand) -> Result<AgentEvent, Self::Error> {
        let orchestrator_node::model::CommandOutcome::Completed { mut event } =
            common::outcome(command)
        else {
            unreachable!()
        };
        event.node_id = id(999);
        Ok(*event)
    }
}

#[tokio::test]
async fn worker_rejects_duplicate_registration_and_misrouted_results() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    assert!(matches!(
        worker::run(
            &fixture.journal,
            id(4),
            &instance().to_string(),
            WrongAdapter
        )
        .await,
        Err(NodeError::Rejected(_))
    ));
    assert!(
        worker::run(
            &fixture.journal,
            id(4),
            &instance().to_string(),
            WrongAdapter
        )
        .await
        .is_err()
    );
    assert_eq!(
        fixture
            .journal
            .command(id(6))
            .await
            .expect("receipt")
            .expect("saved")
            .status,
        CommandStatus::Executing
    );
    fixture.journal.close().await;
}
