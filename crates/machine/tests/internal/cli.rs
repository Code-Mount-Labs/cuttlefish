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

struct LimitedWriter(usize);
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0 == 0 {
            return Err(io::Error::other("output unavailable"));
        }
        let size = bytes.len().min(self.0);
        self.0 -= size;
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("flush failed"))
    }
}
#[test]
fn prompts_propagate_read_write_and_flush_failures() {
    for limit in 0..40 {
        assert!(
            ask(
                &mut &b"ok\n"[..],
                &mut LimitedWriter(limit),
                "Question",
                Some("default")
            )
            .is_err()
        );
    }
    assert!(ask(&mut &b"\xff\n"[..], &mut Vec::new(), "Question", None).is_err());
}
#[tokio::test]
async fn setup_propagates_failures_at_every_input_and_output_boundary() {
    let (origin, server) = registry(vec![InstallationRole::Router], true).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    let listen = "127.0.0.1:8080".parse().expect("address");
    for input in [
        String::new(),
        format!("{}\n", Uuid::from_u128(1)),
        answers(""),
        answers("1\n"),
        answers("1\nname\n"),
        answers("1\nname\ninvalid\n"),
    ] {
        assert!(
            setup(
                &machine,
                &origin,
                listen,
                &mut input.as_bytes(),
                &mut Vec::new()
            )
            .await
            .is_err()
        );
    }
    assert!(
        setup(&machine, "invalid", listen, &mut &b""[..], &mut Vec::new())
            .await
            .is_err()
    );
    server.abort();
    assert!(
        setup(
            &machine,
            &origin,
            listen,
            &mut answers("1\nname\n\n").as_bytes(),
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    machine.close().await;
}
#[tokio::test]
async fn run_requires_a_home_and_reports_storage_and_setup_errors() {
    let cli = || Cli {
        state_dir: None,
        command: Mode::Serve {
            listen: "127.0.0.1:0".parse().expect("listen"),
        },
    };
    assert!(
        run_with(
            cli(),
            None,
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    let dir = tempfile::tempdir().expect("directory");
    std::fs::write(dir.path().join(".cuttlefish"), "blocked").expect("block directory");
    assert!(
        run_with(
            cli(),
            Some(dir.path().into()),
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    let mut args = cli();
    args.state_dir = Some(dir.path().join("new"));
    args.command = Mode::Setup {
        platform: "invalid".into(),
        listen: "127.0.0.1:0".parse().expect("listen"),
    };
    assert!(
        run_with(
            args,
            None,
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn dashboard_reports_corrupt_state_and_broken_output_without_panicking() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    assert!(print_dashboard(&machine, &mut Vec::new()).await.is_err());
    crate::tests::initialize(&machine).await;
    assert!(
        print_dashboard(&machine, &mut LimitedWriter(0))
            .await
            .is_err()
    );
    std::fs::create_dir(dir.path().join("setup.token")).expect("unreadable token");
    assert!(print_dashboard(&machine, &mut Vec::new()).await.is_err());
    std::fs::remove_dir(dir.path().join("setup.token")).expect("unblock token");
    let mut output = Vec::new();
    print_dashboard(&machine, &mut output)
        .await
        .expect("dashboard");
    let prefix = String::from_utf8(output)
        .expect("text")
        .find("One-time")
        .expect("prefix");
    assert!(
        print_dashboard(&machine, &mut LimitedWriter(prefix))
            .await
            .is_err()
    );
    sqlx::query("DROP TABLE local_admin")
        .execute(&machine.pool)
        .await
        .expect("drop admin");
    assert!(print_dashboard(&machine, &mut Vec::new()).await.is_err());
    machine.close().await;
    let mut cli = Cli {
        state_dir: Some(dir.path().into()),
        command: Mode::Serve {
            listen: "127.0.0.1:0".parse().expect("listen"),
        },
    };
    assert!(
        run_with(
            cli,
            None,
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    crate::tests::initialize(&machine).await;
    machine.close().await;
    drop(machine);
    cli = Cli {
        state_dir: Some(dir.path().into()),
        command: Mode::Serve {
            listen: "127.0.0.1:0".parse().expect("listen"),
        },
    };
    assert!(
        run_with(
            cli,
            None,
            || Err(io::Error::other("executable missing")),
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
}

struct OutputLimit(usize);
impl Write for OutputLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0 == 0 {
            return Err(io::Error::other("output disconnected"));
        }
        let size = self.0.min(bytes.len());
        self.0 -= size;
        Ok(size)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn setup_reports_durable_identity_and_output_failures() {
    assert!(machine_name(" ".into()).is_err());
    let (origin, server) = registry(vec![InstallationRole::Router], true).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    let input = answers("1\nname\n\n");
    let listen = "127.0.0.1:8080".parse().expect("listen");
    let mut output = Vec::new();
    setup(
        &machine,
        &origin,
        listen,
        &mut input.as_bytes(),
        &mut output,
    )
    .await
    .expect("setup");
    let text = String::from_utf8(output).expect("output");
    for marker in ["1. Router", "Registration will", "Open the dashboard"] {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
            .await
            .expect("machine");
        let limit = text.find(marker).expect("marker");
        assert!(
            setup(
                &machine,
                &origin,
                listen,
                &mut input.as_bytes(),
                &mut OutputLimit(limit)
            )
            .await
            .is_err()
        );
        machine.close().await;
    }
    assert!(
        setup(
            &machine,
            &origin,
            listen,
            &mut input.as_bytes(),
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    machine.close().await;
    server.abort();
}

#[tokio::test]
async fn configured_startup_and_successful_registration_preserve_identity() {
    let (origin, server) = registry(vec![InstallationRole::Router], false).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
        .await
        .expect("machine");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let listen = listener.local_addr().expect("address");
    setup(
        &machine,
        &origin,
        listen,
        &mut answers("1\nname\n\n").as_bytes(),
        &mut Vec::new(),
    )
    .await
    .expect("registered");
    machine
        .setup_admin(
            &machine.bootstrap_token().expect("token"),
            crate::auth::Credentials {
                username: "operator".into(),
                password: "long-enough-password".into(),
            },
        )
        .await
        .expect("admin");
    let mut text = Vec::new();
    print_dashboard(&machine, &mut text)
        .await
        .expect("existing admin");
    assert!(!String::from_utf8(text).expect("text").contains("One-time"));
    machine.close().await;
    drop(machine);
    let cli = Cli {
        state_dir: Some(dir.path().into()),
        command: Mode::Setup {
            platform: origin,
            listen,
        },
    };
    assert!(
        run_with(
            cli,
            None,
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await
        .is_err()
    );
    server.abort();
}
#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn cli_installs_or_reuses_docker_before_loading_local_state() {
    let cli = Cli {
        state_dir: None,
        command: Mode::InstallDocker,
    };
    run_with(
        cli,
        None,
        std::env::current_exe,
        &mut &b""[..],
        &mut Vec::new(),
    )
    .await
    .expect("existing Docker");
}

#[tokio::test]
async fn serving_an_empty_state_directory_requires_setup() {
    let dir = tempfile::tempdir().expect("directory");
    let cli = Cli {
        state_dir: Some(dir.path().into()),
        command: Mode::Serve {
            listen: "127.0.0.1:0".parse().expect("listen"),
        },
    };
    assert!(matches!(
        run_with(
            cli,
            None,
            std::env::current_exe,
            &mut &b""[..],
            &mut Vec::new()
        )
        .await,
        Err(Error::Invalid("installation_not_initialized"))
    ));
}

#[tokio::test]
async fn setup_persists_managed_choice_and_reports_journal_failures() {
    let (origin, server) = registry(vec![InstallationRole::Router], true).await;
    for fault in [
        "",
        "CREATE TRIGGER fail BEFORE UPDATE OF local_origin ON remote_access BEGIN SELECT RAISE(ABORT,'write failure'); END",
        "CREATE TRIGGER fail BEFORE UPDATE OF choice ON remote_access BEGIN SELECT RAISE(ABORT,'write failure'); END",
    ] {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(DockerBackend::new(dir.path())))
            .await
            .expect("machine");
        sqlx::raw_sql(fault)
            .execute(&machine.pool)
            .await
            .expect("fault");
        let input = answers("1\nname\nmanaged\n");
        let listen = "127.0.0.1:8080".parse().expect("listen");
        let result = setup(
            &machine,
            &origin,
            listen,
            &mut input.as_bytes(),
            &mut Vec::new(),
        )
        .await;
        assert_eq!(result.is_ok(), fault.is_empty());
        if fault.is_empty() {
            assert!(matches!(
                machine.remote_status().await.expect("choice").choice,
                orchestrator_protocol::remote::RemoteChoice::Managed {}
            ));
        }
        machine.close().await;
    }
    server.abort();
}
