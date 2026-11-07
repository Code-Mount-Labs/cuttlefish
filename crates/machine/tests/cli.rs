//! Exercise the distributed binary's actual setup and graceful restart paths.
#![allow(clippy::expect_used)]
#[cfg(unix)]
use axum::{Json, Router, routing::post};
#[cfg(unix)]
use serde_json::{Value, json};
use std::process::Command;
#[cfg(any(unix, feature = "docker-tests"))]
use std::process::Stdio;
#[cfg(unix)]
use std::{io::Write, time::Duration};

#[test]
fn cli_help_and_missing_setup_report_clear_failures() {
    let binary = env!("CARGO_BIN_EXE_cuttlefish");
    assert!(
        Command::new(binary)
            .arg("--help")
            .output()
            .expect("help")
            .status
            .success()
    );
    assert!(
        !Command::new(binary)
            .args(["serve", "--listen", "invalid"])
            .output()
            .expect("invalid")
            .status
            .success()
    );
    assert!(
        !Command::new(binary)
            .arg("serve")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .output()
            .expect("missing home")
            .status
            .success()
    );
    let dir = tempfile::tempdir().expect("directory");
    assert!(
        !Command::new(binary)
            .arg("serve")
            .env("HOME", dir.path())
            .env("USERPROFILE", dir.path())
            .output()
            .expect("missing identity")
            .status
            .success()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn installed_binary_sets_up_and_resumes_without_replacing_identity() {
    let app=Router::new().route("/api/v1/installations/preflight",post(||async{Json(json!({"roles":["router"],"router_url":null}))}))
        .route("/api/v1/installations",post(|Json(value):Json<Value>|async move{Json(json!({"group_id":value["group_id"],"installation_id":value["installation_id"],"router_url":value["router_url"]}))}))
        .route("/api/v1/installations/heartbeat",post(||async{Json(json!({"ok":true}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("registry");
    let platform = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("registry");
    });
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").expect("local port");
    let address = reserve.local_addr().expect("address");
    drop(reserve);
    let directory = tempfile::tempdir().expect("directory");
    let mut saved = None;
    for mode in ["setup", "setup", "serve"] {
        let log = tempfile::tempfile().expect("log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_cuttlefish"));
        command
            .arg("--state-dir")
            .arg(directory.path())
            .arg(mode)
            .arg("--listen")
            .arg(address.to_string())
            .stdin(Stdio::piped())
            .stdout(log.try_clone().expect("clone"))
            .stderr(log);
        if mode == "setup" {
            command.args(["--platform", &platform]);
        }
        let mut child = command.spawn().expect("start");
        if saved.is_none() {
            writeln!(
                child.stdin.take().expect("input"),
                "00000000-0000-0000-0000-000000000001\n{}\n1\nCLI test\nlocal",
                "a".repeat(64)
            )
            .expect("answers");
        }
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !client
            .get(format!("http://{address}/api/local/status"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            assert!(
                child.try_wait().expect("status").is_none(),
                "process exited early"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "listener did not start"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        stop_child(&mut child).await;
        let machine = cuttlefish_machine::state::Machine::open(
            directory.path(),
            std::sync::Arc::new(cuttlefish_machine::docker::DockerBackend::new(
                directory.path(),
            )),
        )
        .await
        .expect("journal");
        let identity = machine
            .identity()
            .await
            .expect("identity")
            .registration
            .installation_id;
        if let Some(original) = saved {
            assert_eq!(identity, original);
        } else {
            saved = Some(identity);
            let token = machine.bootstrap_token().expect("bootstrap");
            machine
                .setup_admin(
                    &token,
                    cuttlefish_machine::auth::Credentials {
                        username: "operator".into(),
                        password: "long-enough-password".into(),
                    },
                )
                .await
                .expect("admin");
        }
        machine.close().await;
    }
    server.abort();
}

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn dependency_installation_reuses_the_running_test_docker() {
    assert!(
        Command::new("docker")
            .arg("info")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("test Docker")
            .success()
    );
    cuttlefish_machine::dependencies::install_docker(false)
        .await
        .expect("already installed");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_cuttlefish"))
            .arg("install-docker")
            .status()
            .expect("dependency command")
            .success()
    );
}

#[cfg(unix)]
async fn stop_child(child: &mut std::process::Child) {
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("signal")
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("exit") {
            assert!(status.success());
            break;
        }
        assert!(std::time::Instant::now() < deadline, "shutdown timed out");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
