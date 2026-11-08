//! Real child-process tests. The supervisor is actually killed and restarted.
#![allow(clippy::expect_used)]

use crate::common;

use std::{path::Path, process::Stdio, time::Duration};

use common::{cancel, id, identity, send, start};
use orchestrator_node::{
    cli::{Request, Response},
    journal::Journal,
    model::{CommandStatus, SessionStatus, WorkspaceMount},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const BINARY: &str = env!("CARGO_BIN_EXE_orchestrator-node");

struct Cleanup(TempDir);

impl Drop for Cleanup {
    fn drop(&mut self) {
        // Last-resort test cleanup also runs on assertion failure. It requests
        // shutdown through the journal, never guesses which PID to terminate.
        let _ = std::process::Command::new(BINARY)
            .arg("stop")
            .arg("--state-dir")
            .arg(self.0.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(150));
    }
}

struct Supervisor {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Supervisor {
    async fn submit(&mut self, command: orchestrator_protocol::agent::AgentCommand) {
        assert!(matches!(
            self.request(&Request::Submit {
                command: Box::new(command)
            })
            .await,
            Response::Receipt { .. }
        ));
    }

    async fn start(path: &Path) -> Self {
        let mut child = Command::new(BINARY)
            .arg("run")
            .arg("--state-dir")
            .arg(path)
            .arg("--group")
            .arg(identity().group_id.to_string())
            .arg("--node")
            .arg(identity().node_id.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("start supervisor");
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut result = Self {
            child,
            input,
            output,
        };
        assert!(matches!(result.read().await, Response::Ready { .. }));
        result
    }

    async fn read(&mut self) -> Response {
        let mut line = String::new();
        let count = tokio::time::timeout(Duration::from_secs(10), self.output.read_line(&mut line))
            .await
            .expect("response deadline")
            .expect("response line");
        assert_ne!(count, 0, "supervisor closed stdout");
        serde_json::from_str(&line).expect("typed response")
    }

    async fn request(&mut self, request: &Request) -> Response {
        let mut bytes = serde_json::to_vec(request).expect("request");
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.expect("send");
        self.input.flush().await.expect("flush");
        self.read().await
    }

    async fn stop(mut self) {
        assert!(matches!(
            self.request(&Request::Shutdown).await,
            Response::Bye
        ));
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .expect("exit deadline")
                .expect("exit")
                .success()
        );
    }
}

async fn completed(journal: &Journal, command: u128) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = journal
                .command(id(command))
                .await
                .expect("read command")
                .expect("saved command");
            if record.status == CommandStatus::Completed {
                break;
            }
            assert!(
                !matches!(
                    record.status,
                    CommandStatus::Rejected | CommandStatus::Uncertain
                ),
                "unexpected result: {record:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("specialist completion deadline");
}

#[tokio::test]
async fn original_specialist_survives_both_abrupt_and_orderly_supervisor_restarts() {
    let cleanup = Cleanup(tempfile::tempdir().expect("state directory"));
    let path = cleanup.0.path();
    let mut supervisor = Supervisor::start(path).await;
    assert!(matches!(
        supervisor
            .request(&Request::Mount {
                mount: WorkspaceMount {
                    workspace_id: id(5),
                    path: path.to_path_buf()
                }
            })
            .await,
        Response::Mounted
    ));
    supervisor.submit(start()).await;
    let journal = Journal::attach(&path.join("node.sqlite"))
        .await
        .expect("inspection connection");
    completed(&journal, 6).await;
    let original = journal
        .session(id(4))
        .await
        .expect("session")
        .expect("worker");
    assert!(original.pid.is_some());
    assert_ne!(original.pid, supervisor.child.id().map(i64::from));

    // This terminates the actual supervisor process, not an in-memory facade.
    supervisor.child.kill().await.expect("kill supervisor");
    let mut replacement = Supervisor::start(path).await;
    let same = journal
        .session(id(4))
        .await
        .expect("session")
        .expect("worker survived");
    assert_eq!(same.pid, original.pid);
    assert_eq!(same.instance_id, original.instance_id);
    assert_eq!(same.state, SessionStatus::Running);
    replacement.submit(start()).await;
    replacement.submit(send(11)).await;
    completed(&journal, 11).await;
    replacement.stop().await;

    let mut third = Supervisor::start(path).await;
    third.submit(send(12)).await;
    completed(&journal, 12).await;
    let still_same = journal
        .session(id(4))
        .await
        .expect("session")
        .expect("same worker");
    assert_eq!(still_same.pid, original.pid);
    assert_eq!(still_same.instance_id, original.instance_id);
    assert_eq!(
        journal
            .pending_events()
            .await
            .expect("one event per command")
            .len(),
        3
    );
    third.submit(cancel(13)).await;
    completed(&journal, 13).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        journal
            .session(id(4))
            .await
            .expect("session")
            .expect("stopped")
            .state,
        SessionStatus::Stopped
    );
    third.stop().await;
    journal.close().await;
}

#[tokio::test]
async fn real_worker_errors_and_operator_stop_have_bounded_exits() {
    let cleanup = Cleanup(tempfile::tempdir().expect("directory"));
    let mut supervisor = Supervisor::start(cleanup.0.path()).await;
    supervisor
        .request(&Request::Mount {
            mount: WorkspaceMount {
                workspace_id: id(5),
                path: cleanup.0.path().to_path_buf(),
            },
        })
        .await;
    supervisor
        .request(&Request::Submit {
            command: Box::new(start()),
        })
        .await;
    let journal = Journal::attach(&cleanup.0.path().join("node.sqlite"))
        .await
        .expect("journal");
    completed(&journal, 6).await;
    let original = journal
        .session(id(4))
        .await
        .expect("session")
        .expect("exists");
    let duplicate = Command::new(BINARY)
        .arg("worker")
        .arg("--journal")
        .arg(journal.path())
        .arg("--session")
        .arg(id::<orchestrator_protocol::ids::SessionId>(4).to_string())
        .arg("--instance")
        .arg(&original.instance_id)
        .output()
        .await
        .expect("duplicate worker");
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("no longer owns"));
    let stopped = Command::new(BINARY)
        .arg("stop")
        .arg("--state-dir")
        .arg(cleanup.0.path())
        .status()
        .await
        .expect("request stop");
    assert!(stopped.success());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if journal
                .session(id(4))
                .await
                .expect("session")
                .expect("saved")
                .state
                == SessionStatus::Stopped
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("worker stops");
    supervisor.stop().await;
    journal.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn local_subscription_specialists_keep_sessions_across_supervisor_restart() {
    use orchestrator_node::execution::ExecutionConfig;
    use orchestrator_protocol::execution::{LocalProvider, ProviderWorkspace};
    use std::os::unix::fs::PermissionsExt;
    for provider in [LocalProvider::Codex, LocalProvider::Claude] {
        let cleanup = Cleanup(tempfile::tempdir().expect("directory"));
        let path = cleanup.0.path().canonicalize().expect("path");
        let executable = path.join("provider");
        std::fs::write(&executable, PROVIDER_FIXTURE).expect("fixture CLI");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let instructions = path.join("specialist.md");
        std::fs::write(&instructions, "Do the assigned task.").expect("prompt");
        let mut supervisor = Supervisor::start(&path).await;
        let journal = Journal::attach(&path.join("node.sqlite"))
            .await
            .expect("journal");
        journal
            .configure_execution(&ExecutionConfig {
                workspace: ProviderWorkspace {
                    workspace_id: id(5),
                    name: "Recovery fixture".into(),
                    provider,
                    allow_edits: true,
                },
                executable,
                directory: path.clone(),
                instructions,
            })
            .await
            .expect("configure");
        supervisor.submit(start()).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !path.join("entered").exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("provider active");
        let original = journal
            .session(id(4))
            .await
            .expect("session")
            .expect("worker");
        let binding = journal
            .provider_binding(id(4))
            .await
            .expect("binding")
            .expect("persisted before effects");
        supervisor
            .child
            .kill()
            .await
            .expect("kill supervisor during task");
        let mut replacement = Supervisor::start(&path).await;
        assert_eq!(
            journal
                .session(id(4))
                .await
                .expect("session")
                .expect("worker")
                .pid,
            original.pid
        );
        assert_eq!(
            journal.provider_binding(id(4)).await.expect("binding"),
            Some(binding.clone())
        );
        std::fs::write(path.join("release"), "").expect("release provider");
        completed(&journal, 6).await;
        replacement.submit(send(11)).await;
        completed(&journal, 11).await;
        assert_eq!(
            journal.provider_binding(id(4)).await.expect("binding"),
            Some(binding)
        );
        let invocations = std::fs::read_to_string(path.join("invocations")).expect("invocations");
        assert_eq!(
            invocations.lines().count(),
            2,
            "one provider invocation per turn"
        );
        match provider {
            LocalProvider::Codex => assert!(
                std::fs::read_to_string(path.join("requests"))
                    .expect("requests")
                    .contains("thread/resume")
            ),
            LocalProvider::Claude => {
                assert!(invocations.contains("--resume 00000000-0000-0000-0000-000000000004"));
            }
        }
        replacement.submit(cancel(12)).await;
        completed(&journal, 12).await;
        replacement.stop().await;
        journal.close().await;
    }
}

#[cfg(unix)]
const PROVIDER_FIXTURE: &str = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> invocations
if [ "$1" = app-server ]; then
  read -r initialize
  printf '%s\n' '{"id":1,"result":{}}'
  read -r initialized
  read -r thread
  printf '%s\n' "$thread" >> requests
  printf '%s\n' '{"id":2,"result":{"thread":{"id":"durable-codex-session"}}}'
  read -r turn
  printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn-1"}}}'
  touch entered
  while [ ! -f release ]; do sleep 0.05; done
  printf '%s\n' '{"method":"item/completed","params":{"threadId":"durable-codex-session","turnId":"turn-1","item":{"type":"agentMessage","text":"Verified task"}}}' '{"method":"turn/completed","params":{"threadId":"durable-codex-session","turn":{"id":"turn-1","status":"completed"}}}'
else
  cat >/dev/null
  touch entered
  while [ ! -f release ]; do sleep 0.05; done
  printf '%s\n' '{"type":"result","session_id":"00000000-0000-0000-0000-000000000004","subtype":"success","is_error":false,"result":"Verified task"}'
fi
"#;
