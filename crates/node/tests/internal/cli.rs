#![allow(clippy::expect_used)]

use super::*;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

#[path = "../suites/cli.rs"]
mod cases;
use crate::test_support as common;
use cases::Noop;

#[tokio::test]
async fn permission_operator_commands_validate_credentials_and_preserve_reviews() {
    let fixture = crate::permissions::tests::setup().await;
    crate::permissions::tests::pending(&fixture).await;
    let request = crate::permissions::tests::request();
    let key = fixture.temp.path().join("reviewer-key");
    let key = key.to_str().expect("path");
    let state = fixture.temp.path().to_str().expect("path");
    let reviewer = common::id::<orchestrator_protocol::ids::ReviewerId>(202).to_string();
    let permission = request.permission_id.to_string();
    let decision = common::id::<CommandId>(201).to_string();
    let digest = request.digest();
    let cli = |state: &str, args: &[&str]| {
        Cli::try_parse_from(
            ["node", "permissions", "--state-dir", state]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .expect("CLI")
    };
    let enroll = ["reviewer", "--reviewer", &reviewer, "--token-file", key];
    assert!(
        test_run(cli("/missing-state", &enroll), std::env::current_exe)
            .await
            .is_err()
    );
    assert!(
        test_run(cli(state, &enroll), std::env::current_exe)
            .await
            .is_err()
    );
    for invalid in [vec![255], b"invalid".to_vec(), vec![b'a'; 100]] {
        std::fs::write(key, invalid).expect("bad key");
        assert!(
            test_run(cli(state, &enroll), std::env::current_exe)
                .await
                .is_err()
        );
    }
    std::fs::write(key, "b".repeat(64)).expect("key");
    test_run(cli(state, &enroll), std::env::current_exe)
        .await
        .expect("enroll");
    for args in [
        vec!["pending"],
        vec!["pending", "--after", &permission],
        vec!["inspect", "--permission", &permission],
    ] {
        test_run(cli(state, &args), std::env::current_exe)
            .await
            .expect("inspect");
    }
    let review = [
        "decide",
        "--token-file",
        key,
        "--permission",
        &permission,
        "--decision",
        &decision,
        "--digest",
        &digest,
    ];
    test_run(cli(state, &review), std::env::current_exe)
        .await
        .expect("deny");
    test_run(cli(state, &review), std::env::current_exe)
        .await
        .expect("retry");
    let mut allow = review.to_vec();
    allow.push("--allow");
    assert!(
        test_run(cli(state, &allow), std::env::current_exe)
            .await
            .is_err()
    );
    let mut disable = enroll.to_vec();
    disable.push("--disabled");
    test_run(cli(state, &disable), std::env::current_exe)
        .await
        .expect("disable");
    assert!(
        test_run(cli(state, &review), std::env::current_exe)
            .await
            .is_err()
    );
    assert!(
        test_run(
            cli(
                state,
                &[
                    "inspect",
                    "--permission",
                    &common::id::<SessionId>(999).to_string()
                ]
            ),
            std::env::current_exe
        )
        .await
        .is_err()
    );
    fixture.journal.close().await;
}

#[tokio::test]
async fn permission_operator_storage_and_decision_file_failures_are_explicit() {
    let fixture = crate::permissions::tests::setup().await;
    crate::permissions::tests::pending(&fixture).await;
    let key = fixture.temp.path().join("token");
    std::fs::write(&key, "b".repeat(64)).expect("key");
    let state = fixture.temp.path().to_str().expect("path");
    let cli = |args: &[&str]| {
        Cli::try_parse_from(
            ["node", "permissions", "--state-dir", state]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .expect("CLI")
    };
    let reviewer = common::id::<orchestrator_protocol::ids::ReviewerId>(999).to_string();
    assert!(
        test_run(
            cli(&[
                "reviewer",
                "--reviewer",
                &reviewer,
                "--token-file",
                key.to_str().expect("key")
            ]),
            std::env::current_exe
        )
        .await
        .is_err()
    );
    let request = crate::permissions::tests::request();
    assert!(
        test_run(
            cli(&[
                "decide",
                "--permission",
                &request.permission_id.to_string(),
                "--decision",
                &common::id::<CommandId>(999).to_string(),
                "--digest",
                &request.digest(),
                "--token-file",
                "/missing-reviewer-token"
            ]),
            std::env::current_exe
        )
        .await
        .is_err()
    );
    let raw = fixture.raw().await;
    sqlx::query("DROP TABLE runtime_permissions")
        .execute(&raw)
        .await
        .expect("missing table");
    assert!(
        test_run(cli(&["pending"]), std::env::current_exe)
            .await
            .is_err()
    );
    raw.close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn dispatch_propagates_storage_errors() {
    let temp = tempfile::tempdir().expect("directory");
    let identity = NodeIdentity {
        group_id: "00000000-0000-0000-0000-000000000001"
            .parse()
            .expect("group"),
        node_id: "00000000-0000-0000-0000-000000000002"
            .parse()
            .expect("node"),
    };
    let mut runtime = NodeRuntime::open(temp.path(), identity, Noop)
        .await
        .expect("runtime");
    runtime.journal().close().await;
    let requests = [
        Request::Mount {
            mount: WorkspaceMount {
                workspace_id: "00000000-0000-0000-0000-000000000005"
                    .parse()
                    .expect("workspace"),
                path: temp.path().to_path_buf(),
            },
        },
        Request::Session {
            session_id: "00000000-0000-0000-0000-000000000004"
                .parse()
                .expect("session"),
        },
        Request::Command {
            command_id: "00000000-0000-0000-0000-000000000006"
                .parse()
                .expect("command"),
        },
        Request::Events,
        Request::Acknowledge { sequence: 1 },
    ];
    for request in requests {
        assert!(dispatch(&mut runtime, request).await.is_err());
    }
}

struct TestResponse(bool);
impl Serialize for TestResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0 {
            Err(serde::ser::Error::custom("serialization failed"))
        } else {
            serializer.serialize_unit()
        }
    }
}

#[tokio::test]
async fn serialization_failure_writes_no_partial_frame() {
    let mut output = Vec::new();
    assert!(matches!(
        write_response(&mut output, &TestResponse(true)).await,
        Err(NodeError::Json(_))
    ));
    assert!(output.is_empty());
    for stage in 0..4 {
        let mut output: Box<dyn AsyncWrite + Unpin> = match stage {
            1 => Box::new(cases::BrokenWriter { fail_write: true }),
            2 => Box::new(cases::BrokenWriter { fail_write: false }),
            _ => Box::new(Vec::new()),
        };
        let result = write_response(&mut output, &TestResponse(stage == 0)).await;
        assert_eq!(result.is_ok(), stage == 3);
    }
}

struct DisconnectAfterReady(bool);
impl AsyncWrite for DisconnectAfterReady {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.0 {
            return Poll::Ready(Err(io::Error::other("disconnected")));
        }
        self.0 = true;
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn response_stream_failure_after_startup_is_reported() {
    let temp = tempfile::tempdir().expect("directory");
    let identity = NodeIdentity {
        group_id: "00000000-0000-0000-0000-000000000001"
            .parse()
            .expect("group"),
        node_id: "00000000-0000-0000-0000-000000000002"
            .parse()
            .expect("node"),
    };
    let mut runtime = NodeRuntime::open(temp.path(), identity, Noop)
        .await
        .expect("runtime");
    assert!(
        cases::serve(
            &mut runtime,
            &b"{\"type\":\"shutdown\"}\n"[..],
            DisconnectAfterReady(false)
        )
        .await
        .is_err()
    );
    runtime.journal().close().await;
}

#[tokio::test]
async fn command_bootstrap_failures_are_reported() {
    let temp = tempfile::tempdir().expect("directory");
    let identity = NodeIdentity {
        group_id: "00000000-0000-0000-0000-000000000001"
            .parse()
            .expect("group"),
        node_id: "00000000-0000-0000-0000-000000000002"
            .parse()
            .expect("node"),
    };
    let mode = || Mode::Run {
        state_dir: temp.path().join("file"),
        group: identity.group_id,
        node: identity.node_id,
    };
    assert!(
        test_run(Cli { mode: mode() }, || Err(io::Error::other(
            "executable unavailable"
        )))
        .await
        .is_err()
    );
    std::fs::write(temp.path().join("file"), "file").expect("file");
    assert!(
        test_run(Cli { mode: mode() }, std::env::current_exe)
            .await
            .is_err()
    );
    assert!(
        test_run(
            Cli {
                mode: Mode::Stop {
                    state_dir: temp.path().join("missing")
                }
            },
            std::env::current_exe
        )
        .await
        .is_err()
    );
    assert!(
        test_run(
            Cli {
                mode: Mode::Worker {
                    journal: temp.path().join("missing"),
                    session: "00000000-0000-0000-0000-000000000004"
                        .parse()
                        .expect("session"),
                    instance: "unknown".into(),
                }
            },
            std::env::current_exe
        )
        .await
        .is_err()
    );
}

async fn test_run(cli: Cli, executable: fn() -> io::Result<PathBuf>) -> Result<(), NodeError> {
    run_with_io(
        cli,
        executable,
        &b"{\"type\":\"shutdown\"}\n"[..],
        Vec::new(),
    )
    .await
}

#[tokio::test]
async fn modes_run_with_injected_local_streams() {
    let fixture = common::Fixture::new().await;
    let identity = common::identity();
    test_run(
        Cli {
            mode: Mode::Run {
                state_dir: fixture.temp.path().to_path_buf(),
                group: identity.group_id,
                node: identity.node_id,
            },
        },
        std::env::current_exe,
    )
    .await
    .expect("run then shutdown");
    fixture.accepted().await;
    fixture
        .journal
        .accept(&common::cancel(11), common::instance())
        .await
        .expect("cancel queued");
    test_run(
        Cli {
            mode: Mode::Worker {
                journal: fixture.journal.path().to_path_buf(),
                session: common::id(4),
                instance: common::instance().to_string(),
            },
        },
        std::env::current_exe,
    )
    .await
    .expect("worker executes then cancels");
    test_run(
        Cli {
            mode: Mode::Stop {
                state_dir: fixture.temp.path().to_path_buf(),
            },
        },
        std::env::current_exe,
    )
    .await
    .expect("stop mode");
    fixture.journal.close().await;
}

#[tokio::test]
async fn synchronization_mode_validates_files_and_reaches_one_shot_and_watch_modes() {
    use axum::{Json, Router, routing::post};
    use orchestrator_protocol::sync::{SyncRequest, SyncResponse};
    let fixture = common::Fixture::new().await;
    fixture.completed().await;
    let token = fixture.temp.path().join("token");
    let profile = fixture.temp.path().join("profile.json");
    let mode = |origin: &str, state: &std::path::Path, once| Cli {
        mode: Mode::Sync {
            super_agent: None,
            execute: false,
            state_dir: state.to_path_buf(),
            coordinator: origin.into(),
            token_file: token.clone(),
            profile_file: profile.clone(),
            once,
        },
    };
    let state = fixture.temp.path();
    assert!(
        test_run(mode("invalid", state, true), std::env::current_exe)
            .await
            .is_err()
    );
    std::fs::write(&token, "invalid").expect("invalid token");
    assert!(
        test_run(mode("invalid", state, true), std::env::current_exe)
            .await
            .is_err()
    );
    std::fs::write(&token, "a".repeat(64)).expect("token");
    assert!(
        test_run(mode("invalid", state, true), std::env::current_exe)
            .await
            .is_err()
    );
    std::fs::write(&profile, "invalid JSON").expect("invalid profile");
    assert!(
        test_run(mode("invalid", state, true), std::env::current_exe)
            .await
            .is_err()
    );
    std::fs::write(
        &profile,
        r#"{"platform":"linux","capabilities":["code.rust"],"capacity":4}"#,
    )
    .expect("profile");
    assert!(
        test_run(mode("invalid", state, true), std::env::current_exe)
            .await
            .is_err()
    );
    assert!(
        test_run(
            mode("http://127.0.0.1:1", &state.join("missing"), true),
            std::env::current_exe
        )
        .await
        .is_err()
    );
    assert!(
        test_run(
            mode("http://127.0.0.1:1", state, true),
            std::env::current_exe
        )
        .await
        .is_err()
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let router = Router::new().route(
        "/v1/sync",
        post(|Json(report): Json<SyncRequest>| async move {
            Json(SyncResponse {
                version: report.version,
                acknowledged: report.events.iter().map(|e| e.sequence).collect(),
            })
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server");
    });
    test_run(mode(&origin, state, true), std::env::current_exe)
        .await
        .expect("one batch");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            test_run(mode(&origin, state, false), std::env::current_exe)
        )
        .await
        .is_err()
    );
    server.abort();
    fixture.journal.close().await;
}

#[tokio::test]
async fn execution_mode_owns_the_supervisor_and_supports_one_shot_and_watch() {
    fn missing_executable() -> std::io::Result<PathBuf> {
        Err(std::io::Error::other("missing executable"))
    }
    use axum::{Json, Router, routing::post};
    use orchestrator_protocol::{
        routing::{DispatchBatch, DispatchPoll},
        sync::{SyncRequest, SyncResponse},
    };
    let fixture = common::Fixture::new().await;
    let token = fixture.temp.path().join("token");
    let profile = fixture.temp.path().join("profile");
    std::fs::write(&token, "a".repeat(64)).expect("token");
    std::fs::write(
        &profile,
        r#"{"platform":"linux","capabilities":["code.rust"],"capacity":4}"#,
    )
    .expect("profile");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let router = Router::new()
        .route(
            "/v1/sync",
            post(|Json(report): Json<SyncRequest>| async move {
                Json(SyncResponse {
                    version: report.version,
                    acknowledged: vec![],
                })
            }),
        )
        .route(
            "/v1/dispatch/poll",
            post(|Json(poll): Json<DispatchPoll>| async move {
                Json(DispatchBatch {
                    acknowledged: poll.accepted,
                    command: None,
                })
            }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let mode = |once| Cli {
        mode: Mode::Sync {
            super_agent: None,
            state_dir: fixture.temp.path().to_path_buf(),
            coordinator: origin.clone(),
            token_file: token.clone(),
            profile_file: profile.clone(),
            once,
            execute: true,
        },
    };
    assert!(test_run(mode(true), missing_executable).await.is_err());
    let runtime = NodeRuntime::open(
        fixture.temp.path(),
        fixture.journal.identity(),
        LocalLauncher::new(std::env::current_exe().expect("binary")),
    )
    .await
    .expect("own lock");
    assert!(test_run(mode(true), std::env::current_exe).await.is_err());
    runtime.journal().close().await;
    drop(runtime);
    test_run(mode(true), std::env::current_exe)
        .await
        .expect("once");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            test_run(mode(false), std::env::current_exe)
        )
        .await
        .is_err()
    );
    server.abort();
    fixture.journal.close().await;
}

#[tokio::test]
async fn super_agent_configuration_is_explicit_and_failures_are_reported() {
    let fixture = common::Fixture::new().await;
    let token = fixture.temp.path().join("token");
    let profile = fixture.temp.path().join("profile");
    let settings = fixture.temp.path().join("super.json");
    let key = fixture.temp.path().join("key");
    std::fs::write(&token, "a".repeat(64)).expect("token");
    std::fs::write(
        &profile,
        r#"{"platform":"linux","capabilities":["code.rust"],"capacity":4}"#,
    )
    .expect("profile");
    let cli = |once| Cli {
        mode: Mode::Sync {
            state_dir: fixture.temp.path().to_path_buf(),
            coordinator: "http://127.0.0.1:1".into(),
            token_file: token.clone(),
            profile_file: profile.clone(),
            once,
            execute: true,
            super_agent: Some(settings.clone()),
        },
    };
    assert!(test_run(cli(true), std::env::current_exe).await.is_err());
    std::fs::write(&settings, "invalid").expect("settings");
    assert!(test_run(cli(true), std::env::current_exe).await.is_err());
    let mut configuration = serde_json::json!({"goal_id":"00000000-0000-0000-0000-000000000100","endpoint":"http://127.0.0.1:1","model":"test-model","key_file":key,"turn_limit":10});
    std::fs::write(&settings, configuration.to_string()).expect("settings");
    assert!(test_run(cli(true), std::env::current_exe).await.is_err());
    std::fs::write(&key, "").expect("key");
    assert!(test_run(cli(true), std::env::current_exe).await.is_err());
    std::fs::write(&key, "test-key").expect("key");
    for once in [true, false] {
        assert!(test_run(cli(once), std::env::current_exe).await.is_err());
    }
    configuration["prompt_dir"] = serde_json::json!(fixture.temp.path());
    std::fs::write(&settings, configuration.to_string()).expect("settings");
    assert!(test_run(cli(true), std::env::current_exe).await.is_err());
    std::fs::create_dir(fixture.temp.path().join("super-agent")).expect("directory");
    std::fs::write(
        fixture.temp.path().join("super-agent/planner.md"),
        "reviewed planner prompt",
    )
    .expect("prompt");
    // Valid startup reaches the unavailable router; no model call is made.
    assert!(matches!(
        test_run(cli(true), std::env::current_exe).await,
        Err(NodeError::Transport(_))
    ));
    fixture.journal.close().await;
}
