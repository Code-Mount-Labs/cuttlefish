//! Exercise authenticated updates through the distributed binary, not a mock API.
#![cfg(unix)]
#![allow(clippy::expect_used)]
use cuttlefish_machine::{docker::DockerBackend, state::Machine};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationRole, Registration},
};
use serde_json::json;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn wait_ready(client: &reqwest::Client, base: &str) {
    for _ in 0..200 {
        if client
            .get(format!("{base}/api/local/status"))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        client
            .get(format!("{base}/api/local/status"))
            .send()
            .await
            .is_ok(),
        "test server did not become ready"
    );
}
fn managed_executable(root: &Path) -> std::path::PathBuf {
    let slot = root.join("releases/test");
    std::fs::create_dir_all(&slot).expect("slot");
    let executable = slot.join("cuttlefish");
    std::fs::copy(env!("CARGO_BIN_EXE_cuttlefish"), &executable).expect("copy binary");
    std::fs::write(
        root.join(".cuttlefish-layout"),
        "cuttlefish-release-layout-v1\n",
    )
    .expect("layout");
    std::fs::write(
        slot.join(".release-version"),
        format!("v{}", env!("CARGO_PKG_VERSION")),
    )
    .expect("version");
    std::os::unix::fs::symlink("releases/test", root.join("current")).expect("current");
    executable
}
#[tokio::test]
async fn dashboard_retry_is_durable_and_authenticated_through_real_binary() {
    let temp = tempfile::tempdir().expect("temp");
    let state = temp.path().join("state");
    let exe = managed_executable(&temp.path().join("app"));
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve");
    let address = reserve.local_addr().expect("address");
    drop(reserve);
    let base = format!("http://{address}");
    let machine = Machine::open(&state, Arc::new(DockerBackend::new(&state)))
        .await
        .expect("machine");
    let registration = Registration {
        group_id: GroupId::try_from(uuid::Uuid::new_v4()).expect("group"),
        installation_id: InstallationId::random(),
        role: InstallationRole::Router,
        name: "Updater test".to_owned().try_into().expect("name"),
        router_url: Some(base.clone()),
        credential: "a".repeat(64).try_into().expect("credential"),
        invitation: "b".repeat(64).try_into().expect("invitation"),
    };
    machine
        .initialize(&registration, "http://127.0.0.1:1", &base)
        .await
        .expect("initialize");
    let token = machine.bootstrap_token().expect("token");
    machine.close().await;
    drop(machine);
    let mut process = Process(
        Command::new(&exe)
            .args([
                "--state-dir",
                state.to_str().expect("path"),
                "serve",
                "--listen",
                &address.to_string(),
            ])
            .env("HOME", temp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("server"),
    );
    let client = reqwest::Client::new();
    wait_ready(&client, &base).await;
    let response = client.post(format!("{base}/api/local/setup")).header("origin",&base)
        .json(&json!({"token":token.expose(),"username":"operator","password":"private-test-password"})).send().await.expect("setup");
    assert!(response.status().is_success());
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("cookie")
        .to_owned();
    let journal = state.join("updates");
    std::fs::create_dir_all(&journal).expect("journal");
    let id = uuid::Uuid::new_v4();
    let saved = json!({"available":null,"checked":true,"check_error":null,"job":{"id":id,"version":format!("v{}",env!("CARGO_PKG_VERSION")),"phase":"restart_required","restart_only":false,"error":null}});
    std::fs::write(
        journal.join("status.json"),
        serde_json::to_vec(&saved).expect("json"),
    )
    .expect("record");
    let response = client
        .post(format!("{base}/api/local/updates"))
        .header("origin", &base)
        .header("cookie", &cookie)
        .json(&json!({"action":"retry"}))
        .send()
        .await
        .expect("retry");
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let result: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(result["job"]["id"], id.to_string());
    for _ in 0..100 {
        let result: serde_json::Value = client
            .get(format!("{base}/api/local/updates"))
            .header("cookie", &cookie)
            .send()
            .await
            .expect("status")
            .json()
            .await
            .expect("json");
        if result["job"]["phase"] == "restart_required" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal.join("status.json")).expect("record"))
            .expect("JSON");
    assert_eq!(persisted["job"]["phase"], "restart_required");
    assert_eq!(persisted["job"]["id"], id.to_string());
    Command::new("kill")
        .args(["-INT", &process.0.id().to_string()])
        .status()
        .expect("signal");
    assert!(process.0.wait().expect("wait").success());
}
