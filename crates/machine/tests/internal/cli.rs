#![allow(clippy::expect_used)]
use super::*;
#[test]
fn interactive_values_are_bounded_required_and_use_explicit_defaults() {
    let mut output = Vec::new();
    assert_eq!(
        ask(&mut &b" answer \n"[..], &mut output, "Question", None).expect("answer"),
        "answer"
    );
    assert_eq!(
        ask(&mut &b"\n"[..], &mut output, "Question", Some("default")).expect("default"),
        "default"
    );
    assert!(ask(&mut &b""[..], &mut output, "Question", None).is_err());
    assert!(ask(&mut &b"\n"[..], &mut output, "Question", None).is_err());
    assert!(
        ask(
            &mut "x".repeat(4097).as_bytes(),
            &mut output,
            "Question",
            None
        )
        .is_err()
    );
    assert!(
        String::from_utf8(output)
            .expect("text")
            .contains("Question [default]: ")
    );
    for (role, name) in [
        (InstallationRole::Router, "Router only"),
        (InstallationRole::RouterAgent, "Router + agent"),
        (InstallationRole::Agent, "Agent only"),
    ] {
        assert_eq!(role_name(role), name);
    }
}
#[test]
fn cli_parses_roles_of_setup_resume_and_dependency_installation() {
    for args in [
        vec!["cuttlefish", "setup"],
        vec!["cuttlefish", "serve"],
        vec!["cuttlefish", "install-docker"],
        vec![
            "cuttlefish",
            "--state-dir",
            "/tmp/example",
            "setup",
            "--listen",
            "127.0.0.1:9000",
        ],
    ] {
        let command = Cli::try_parse_from(args).expect("CLI");
        assert!(!format!("{command:?}").is_empty());
    }
    assert!(Cli::try_parse_from(["cuttlefish", "serve", "--listen", "invalid"]).is_err());
}

async fn registry(
    roles: Vec<InstallationRole>,
    reject_registration: bool,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{Json, Router, http::StatusCode, routing::post};
    use serde_json::{Value, json};
    let app=Router::new().route("/api/v1/installations/preflight",post(move||{let roles=roles.clone();async move {Json(json!({"roles":roles,"router_url":null}))}}))
        .route("/api/v1/installations",post(move|Json(value):Json<Value>|async move {(if reject_registration{StatusCode::SERVICE_UNAVAILABLE}else{StatusCode::OK},Json(json!({"group_id":value["group_id"],"installation_id":value["installation_id"],"router_url":"http://127.0.0.1:8080"})))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    (origin, task)
}
fn answers(tail: &str) -> String {
    format!("{}\n{}\n{tail}", Uuid::from_u128(1), "a".repeat(64))
}
#[tokio::test]
async fn setup_saves_identity_once_and_retries_failed_registration_without_new_credentials() {
    for role in [
        InstallationRole::Router,
        InstallationRole::RouterAgent,
        InstallationRole::Agent,
    ] {
        let (origin, server) = registry(vec![role], true).await;
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
            .await
            .expect("machine");
        let mut output = Vec::new();
        setup(
            &machine,
            &origin,
            "127.0.0.1:8080".parse().expect("address"),
            &mut answers("\n\n\n").as_bytes(),
            &mut output,
        )
        .await
        .expect("setup");
        assert_eq!(
            machine
                .identity()
                .await
                .expect("identity")
                .registration
                .role,
            role
        );
        assert!(
            String::from_utf8(output)
                .expect("output")
                .contains("retry in the background")
        );
        machine.close().await;
        server.abort();
    }
}
#[tokio::test]
async fn setup_rejects_invalid_answers_before_registration() {
    let (origin, server) = registry(vec![InstallationRole::Router], false).await;
    let invalid = [
        "not-a-group\n".into(),
        format!("{}\nshort\n", Uuid::from_u128(1)),
        answers("invalid\n"),
        answers("0\n"),
        answers("2\n"),
        answers(&format!("1\n{}\n", "x".repeat(129))),
        answers("1\nmachine\nhttp://public.example\n"),
    ];
    for value in invalid {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
            .await
            .expect("machine");
        assert!(
            setup(
                &machine,
                &origin,
                "127.0.0.1:8080".parse().expect("address"),
                &mut value.as_bytes(),
                &mut Vec::new()
            )
            .await
            .is_err()
        );
        assert!(machine.identity().await.is_err());
        machine.close().await;
    }
    server.abort();
    let (origin, server) = registry(vec![], false).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    assert!(
        setup(
            &machine,
            &origin,
            "127.0.0.1:8080".parse().expect("address"),
            &mut answers("\n").as_bytes(),
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    machine.close().await;
    server.abort();
}
