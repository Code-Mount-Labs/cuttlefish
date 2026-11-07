use crate::{common::Fixture, data};
use std::{process::Stdio, time::Duration};
use tokio::process::Command;
const BINARY: &str = env!("CARGO_BIN_EXE_orchestrator-coordinator");

#[tokio::test]
async fn model_configuration_is_explicit_and_credentials_are_read_from_a_file() {
    let f = Fixture::new().await;
    let temp = tempfile::tempdir().expect("temp");
    let key = temp.path().join("model.key");
    let key = key.to_str().expect("path");
    let args = [
        "serve",
        "--model",
        "test-model",
        "--model-key-file",
        key,
        "--model-endpoint",
        "http://127.0.0.1:1",
    ];
    assert!(!run(&f, &args).await.status.success());
    std::fs::write(key, "\n").expect("empty");
    assert!(!run(&f, &args).await.status.success());
    std::fs::write(key, "test-key").expect("key");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address").to_string();
    let mut bound = args.to_vec();
    bound.extend(["--listen", &address]);
    // Reaching the address-in-use failure proves configuration succeeded.
    let output = run(&f, &bound).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("AddrInUse"));
    let root = temp.path().to_str().expect("root");
    bound.extend(["--prompt-dir", root]);
    assert!(!run(&f, &bound).await.status.success());
    std::fs::create_dir(temp.path().join("router")).expect("directory");
    std::fs::write(
        temp.path().join("router/selection.md"),
        "select an eligible node",
    )
    .expect("prompt");
    let output = run(&f, &bound).await;
    assert!(String::from_utf8_lossy(&output.stderr).contains("AddrInUse"));
    f.close().await;
}

async fn run(f: &Fixture, args: &[&str]) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(BINARY)
            .env("DATABASE_URL", &f.url)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("exit deadline")
    .expect("execute CLI")
}

#[tokio::test]
async fn operator_cli_provisions_serves_and_revokes() {
    let f = Fixture::new().await;
    let temp = tempfile::tempdir().expect("directory");
    let path = temp.path().join("node.token");
    std::fs::write(&path, data::token().expose()).expect("token file");
    let report = data::report();
    let group = report.group_id.to_string();
    let node = report.node_id.to_string();
    let enroll = [
        "enroll",
        "--group",
        &group,
        "--node",
        &node,
        "--token-file",
        path.to_str().expect("path"),
    ];
    assert!(run(&f, &enroll).await.status.success());
    assert!(!run(&f, &enroll).await.status.success());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let mut server = Command::new(BINARY)
        .env("DATABASE_URL", &f.url)
        .args(["serve", "--listen", &address.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("server");
    let client = reqwest::Client::new();
    let url = format!("http://{address}/v1/nodes");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if client
                .get(&url)
                .bearer_auth(data::token().expose())
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("server readiness");
    f.sql("ALTER TABLE nodes ADD CONSTRAINT refuse_revoke CHECK (NOT revoked)")
        .await;
    assert!(
        !run(&f, &["revoke", "--group", &group, "--node", &node])
            .await
            .status
            .success()
    );
    f.sql("ALTER TABLE nodes DROP CONSTRAINT refuse_revoke")
        .await;
    assert!(
        run(&f, &["revoke", "--group", &group, "--node", &node])
            .await
            .status
            .success()
    );
    assert_eq!(
        client
            .get(url)
            .bearer_auth(data::token().expose())
            .send()
            .await
            .expect("request")
            .status(),
        401
    );
    assert!(
        Command::new("kill")
            .args(["-INT", &server.id().expect("owned PID").to_string()])
            .status()
            .await
            .expect("signal owned child")
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), server.wait())
            .await
            .expect("shutdown deadline")
            .expect("exit")
            .success()
    );
    f.close().await;
}

#[tokio::test]
async fn operator_cli_reports_startup_and_credential_errors() {
    let f = Fixture::new().await;
    let report = data::report();
    let group = report.group_id.to_string();
    let node = report.node_id.to_string();
    let temp = tempfile::tempdir().expect("directory");
    let path = temp.path().join("token");
    let file = path.to_str().expect("path");
    let enroll = [
        "enroll",
        "--group",
        &group,
        "--node",
        &node,
        "--token-file",
        file,
    ];
    assert!(!run(&f, &enroll).await.status.success());
    std::fs::write(&path, "malformed").expect("invalid token");
    assert!(!run(&f, &enroll).await.status.success());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("occupy address");
    assert!(
        !run(
            &f,
            &[
                "serve",
                "--listen",
                &listener.local_addr().expect("address").to_string()
            ]
        )
        .await
        .status
        .success()
    );
    assert!(
        !run(&f, &["--database-url", "not a URL", "serve"])
            .await
            .status
            .success()
    );
    f.close().await;
}

#[tokio::test]
async fn catalog_and_learning_cli_require_valid_files_and_exact_reviews() {
    use orchestrator_protocol::collaboration::{GoalAction, LearningDecision};
    let f = Fixture::enrolled().await;
    let temp = tempfile::tempdir().expect("directory");
    let file = temp.path().join("input.json");
    let path = file.to_str().expect("path");
    let group = data::report().group_id.to_string();
    let catalog = ["catalog", "--group", &group, "--file", path];
    assert!(!run(&f, &catalog).await.status.success());
    std::fs::write(&file, "invalid").expect("file");
    assert!(!run(&f, &catalog).await.status.success());
    let mut source = serde_json::to_value(data::release(300)).expect("source");
    for key in ["recipe", "instructions", "evaluation"] {
        let text = source
            .as_object_mut()
            .expect("object")
            .remove(key)
            .expect("text");
        let name = format!("{key}.md");
        std::fs::write(temp.path().join(&name), text.as_str().expect("text")).expect("markdown");
        source[format!("{key}_file")] = serde_json::json!(name);
    }
    std::fs::write(&file, source.to_string()).expect("manifest");
    assert!(run(&f, &catalog).await.status.success());
    f.sql("ALTER TABLE capability_releases RENAME COLUMN manifest TO hidden")
        .await;
    assert!(!run(&f, &catalog).await.status.success());
    f.sql("ALTER TABLE capability_releases RENAME COLUMN hidden TO manifest")
        .await;
    let mut view = f
        .store
        .create_goal(&data::token(), &data::goal())
        .await
        .expect("goal");
    for action in [
        GoalAction::Plan {
            tasks: vec![data::task(200)],
        },
        GoalAction::Learn {
            task_id: data::id(202),
            node_id: data::id(2),
            release_id: data::id(300),
            rationale: "learn".to_owned().try_into().expect("text"),
        },
    ] {
        view = f
            .store
            .goal_command(
                &data::token(),
                view.spec.goal_id,
                &data::change(&view, action),
            )
            .await
            .expect("command");
    }
    let goal = view.spec.goal_id.to_string();
    let node = view.owner.to_string();
    let release = data::release(300).release_id.to_string();
    let revision = view.revision.to_string();
    let learning = [
        "learning",
        "--group",
        &group,
        "--goal",
        &goal,
        "--node",
        &node,
        "--release",
        &release,
        "--revision",
        &revision,
        "--decision-file",
        path,
    ];
    std::fs::remove_file(&file).expect("remove");
    assert!(!run(&f, &learning).await.status.success());
    std::fs::write(&file, "invalid").expect("file");
    assert!(!run(&f, &learning).await.status.success());
    std::fs::write(
        &file,
        serde_json::to_vec(&LearningDecision::Approve {}).expect("json"),
    )
    .expect("file");
    assert!(run(&f, &learning).await.status.success());
    assert!(!run(&f, &learning).await.status.success());
    f.close().await;
}
