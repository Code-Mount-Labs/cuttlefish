#![allow(clippy::expect_used)]
use super::*;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Debug, Default)]
struct Commands {
    replies: Mutex<VecDeque<Result<CommandResult, Error>>>,
    calls: Mutex<Vec<Vec<String>>>,
}
impl DockerCommands for Commands {
    fn execute(&self, args: Vec<String>) -> CommandFuture<'_> {
        self.calls.lock().expect("lock").push(args);
        Box::pin(async {
            self.replies
                .lock()
                .expect("lock")
                .pop_front()
                .expect("expected call")
        })
    }
}
fn result(success: bool, json: &Value) -> Result<CommandResult, Error> {
    Ok(CommandResult {
        success,
        stdout: serde_json::to_vec(json)?,
    })
}
fn commands(replies: Vec<Result<CommandResult, Error>>) -> Arc<Commands> {
    Arc::new(Commands {
        replies: Mutex::new(replies.into()),
        ..Commands::default()
    })
}
#[derive(Debug, Default)]
struct Probe(AtomicBool);
impl DatabaseProbe for Probe {
    fn verify<'a>(&'a self, _: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}
fn identity() -> String {
    format!("cuttlefish-pg-{}", "1".repeat(32))
}
fn saved(running: bool) -> Value {
    json!([{"Config":{"Labels":{OWNER:identity()},"Image":POSTGRES_IMAGE,
    "Env":["POSTGRES_USER=postgres","POSTGRES_DB=cuttlefish", format!("POSTGRES_PASSWORD={}","a".repeat(64))]},
    "HostConfig":{"RestartPolicy":{"Name":"unless-stopped"},"PortBindings":{"5432/tcp":[{"HostIp":"127.0.0.1","HostPort":"54329"}]}},
    "Mounts":[{"Type":"volume","Name":identity(),"Destination":"/var/lib/postgresql"}],"State":{"Running":running}}])
}
fn spec() -> DatabaseSpec {
    DatabaseSpec::Managed {
        container: identity(),
        volume: identity(),
        port: 54329,
        password: "b".repeat(64),
        admin_password: "a".repeat(64),
    }
}
#[tokio::test]
async fn installed_docker_never_runs_the_installer() {
    let cmds = commands(vec![result(true, &json!({}))]);
    let installed = AtomicBool::new(false);
    ensure(cmds.as_ref(), async {
        installed.store(true, Ordering::SeqCst);
        Ok(())
    })
    .await
    .expect("ready");
    assert!(!installed.load(Ordering::SeqCst));
    assert_eq!(cmds.calls.lock().expect("calls").len(), 1);
}
#[tokio::test]
async fn absent_or_stopped_docker_installs_then_verifies_and_reports_failure() {
    for first in [result(false, &json!({})), Err(Error::Dependency("missing"))] {
        let cmds = commands(vec![first, result(true, &json!({}))]);
        let installed = AtomicBool::new(false);
        ensure(cmds.as_ref(), async {
            installed.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect("installed");
        assert!(installed.load(Ordering::SeqCst));
        assert_eq!(cmds.calls.lock().expect("calls").len(), 2);
    }
    let cmds = commands(vec![result(false, &json!({}))]);
    assert!(matches!(
        ensure(cmds.as_ref(), async {
            Err(Error::Dependency("administrator_required"))
        })
        .await,
        Err(Error::Dependency("administrator_required"))
    ));
    let cmds = commands(vec![result(false, &json!({})), result(false, &json!({}))]);
    assert!(matches!(
        ensure(cmds.as_ref(), async { Ok(()) }).await,
        Err(Error::Dependency(
            "docker_setup_requires_administrator_or_desktop_setup"
        ))
    ));
    let cmds = commands(vec![
        result(false, &json!({})),
        Err(Error::Dependency("still_missing")),
    ]);
    assert!(ensure(cmds.as_ref(), async { Ok(()) }).await.is_err());
}
#[tokio::test]
async fn provisioning_creates_only_named_resources_and_retry_adopts_the_same_storage() {
    let dir = tempfile::tempdir().expect("dir");
    for already_exists in [false, true] {
        let mut replies = vec![result(true, &json!({})), result(already_exists, &json!({}))];
        if !already_exists {
            replies.push(result(true, &json!({})));
        }
        replies.push(result(true, &json!([{"Labels":{OWNER:identity()}}])));
        replies.push(result(already_exists, &json!({})));
        if !already_exists {
            replies.push(result(true, &json!({})));
        }
        replies.push(result(true, &saved(already_exists)));
        if !already_exists {
            replies.push(result(true, &json!({})));
        }
        let cmds = commands(replies);
        let probe = Arc::new(Probe::default());
        let backend = DockerBackend::with_dependencies(dir.path(), cmds.clone(), probe.clone());
        backend.reconcile(&spec()).await.expect("provision");
        assert!(probe.0.load(Ordering::SeqCst));
        let calls = cmds.calls.lock().expect("calls");
        assert!(
            !calls
                .iter()
                .flatten()
                .any(|v| v == "rm" || v == "prune" || v == &"a".repeat(64))
        );
        assert_eq!(
            calls
                .iter()
                .filter(|args| args.contains(&"create".to_owned()))
                .count(),
            if already_exists { 0 } else { 2 }
        );
        assert!(cmds.replies.lock().expect("replies").is_empty());
    }
    let env = std::fs::read_to_string(dir.path().join(format!("{}.env", identity()))).expect("env");
    assert!(env.contains("POSTGRES_USER=postgres"));
}
#[tokio::test]
async fn unrelated_volume_or_container_is_never_modified_or_used() {
    let dir = tempfile::tempdir().expect("dir");
    for invalid_container in [false, true] {
        let mut replies = vec![result(true, &json!({})), result(true, &json!({}))];
        replies.push(result(
            true,
            &json!([{"Labels":{OWNER:if invalid_container{identity()}else{"other".into()}}}]),
        ));
        if invalid_container {
            replies.push(result(true, &json!({})));
            let mut wrong = saved(false);
            wrong[0]["Config"]["Labels"][OWNER] = json!("other");
            replies.push(result(true, &wrong));
        }
        let cmds = commands(replies);
        let probe = Arc::new(Probe::default());
        let backend = DockerBackend::with_dependencies(dir.path(), cmds.clone(), probe.clone());
        assert!(backend.reconcile(&spec()).await.is_err());
        assert!(!probe.0.load(Ordering::SeqCst));
        assert!(
            !cmds
                .calls
                .lock()
                .expect("calls")
                .iter()
                .flatten()
                .any(|v| v == "start" || v == "create" || v == "rm")
        );
    }
}
#[test]
fn every_security_relevant_container_setting_is_checked() {
    for pointer in [
        "/Config/Labels/io.cuttlefish.resource",
        "/Config/Image",
        "/Config/Env",
        "/HostConfig/RestartPolicy/Name",
        "/HostConfig/PortBindings/5432~1tcp",
        "/HostConfig/PortBindings/5432~1tcp/0/HostIp",
        "/HostConfig/PortBindings/5432~1tcp/0/HostPort",
        "/Mounts",
    ] {
        let mut value = saved(true)[0].clone();
        *value.pointer_mut(pointer).expect("pointer") = json!("changed");
        assert!(
            verify_container(&value, &identity(), &identity(), 54329, &"a".repeat(64)).is_err(),
            "{pointer}"
        );
    }
    verify_container(
        &saved(true)[0],
        &identity(),
        &identity(),
        54329,
        &"a".repeat(64),
    )
    .expect("exact");
    for (name, volume, port, password) in [
        ("bad".into(), identity(), 54329, "a".repeat(64)),
        ("cuttlefish-pg-z".into(), identity(), 54329, "a".repeat(64)),
        (
            format!("cuttlefish-pg-{}", "z".repeat(32)),
            identity(),
            54329,
            "a".repeat(64),
        ),
        (identity(), "bad".into(), 54329, "a".repeat(64)),
        (identity(), identity(), 0, "a".repeat(64)),
        (identity(), identity(), 54329, "a".repeat(63)),
        (identity(), identity(), 54329, "z".repeat(64)),
    ] {
        assert!(validate_identity(&name, &volume, port, &password).is_err());
    }
}
#[tokio::test]
async fn external_database_does_not_require_or_install_docker() {
    let dir = tempfile::tempdir().expect("dir");
    let cmds = commands(vec![]);
    let probe = Arc::new(Probe::default());
    let backend = DockerBackend::with_dependencies(dir.path(), cmds.clone(), probe.clone());
    backend
        .reconcile(&DatabaseSpec::Existing {
            url: "postgresql://host/db".into(),
        })
        .await
        .expect("external");
    assert!(probe.0.load(Ordering::SeqCst));
    assert!(cmds.calls.lock().expect("calls").is_empty());
}
#[test]
fn environment_file_is_durable_private_and_rejects_changed_credentials() {
    let dir = tempfile::tempdir().expect("dir");
    let backend =
        DockerBackend::with_dependencies(dir.path(), commands(vec![]), Arc::new(Probe::default()));
    let path = backend
        .environment_file(&identity(), &"a".repeat(64))
        .expect("create");
    assert_eq!(
        backend
            .environment_file(&identity(), &"a".repeat(64))
            .expect("retry"),
        path
    );
    assert!(
        backend
            .environment_file(&identity(), &"b".repeat(64))
            .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[cfg(unix)]
#[tokio::test]
async fn subprocess_output_and_time_are_bounded_and_secrets_are_redacted() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "printf success"]);
    let output = execute_bounded(cmd, Duration::from_secs(2))
        .await
        .expect("command");
    assert!(output.success);
    assert_eq!(output.stdout, b"success");
    assert!(!format!("{output:?}").contains("stdout"));
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "exit 7"]);
    assert!(
        !execute_bounded(cmd, Duration::from_secs(2))
            .await
            .expect("exit")
            .success
    );
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "exec sleep 10"]);
    assert!(matches!(
        execute_bounded(cmd, Duration::from_millis(20)).await,
        Err(Error::Dependency("docker_operation_timed_out"))
    ));
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "head -c 1048577 /dev/zero"]);
    assert!(matches!(
        execute_bounded(cmd, Duration::from_secs(2)).await,
        Err(Error::Dependency("docker_response_too_large"))
    ));
    assert!(
        execute_bounded(
            Command::new("/nonexistent/cuttlefish-command"),
            Duration::from_secs(2)
        )
        .await
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn installer_reuses_working_docker_without_requesting_privileges() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("directory");
    let docker = directory.path().join("docker");
    std::fs::write(&docker, "#!/bin/sh\n[ \"$1\" = info ] || exit 19\nexit 0\n")
        .expect("fake docker");
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(include_str!("../../../../install/docker.sh"))
        .env("PATH", directory.path())
        .output()
        .expect("installer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn stopped_docker_is_started_without_reinstallation_or_group_changes() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("directory");
    let marker = directory.path().join("started");
    let scripts = [
        ("docker", "#!/bin/sh\n[ -f \"$TEST_STARTED\" ]\n"),
        ("uname", "#!/bin/sh\necho Linux\n"),
        ("id", "#!/bin/sh\necho 1000\n"),
        (
            "systemctl",
            "#!/bin/sh\n[ \"$*\" = '--user start docker' ] || exit 22\n: > \"$TEST_STARTED\"\n",
        ),
    ];
    for (name, script) in scripts {
        let path = directory.path().join(name);
        std::fs::write(&path, script).expect("fake command");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("executable");
    }
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(include_str!("../../../../install/docker.sh"))
        .env("PATH", directory.path())
        .env("TEST_STARTED", &marker)
        .output()
        .expect("installer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(marker.exists());
}

#[tokio::test]
async fn every_docker_failure_stops_provisioning_before_database_activation() {
    fn path() -> Vec<Result<CommandResult, Error>> {
        vec![
            result(true, &json!({})),
            result(false, &json!({})),
            result(true, &json!({})),
            result(true, &json!([{"Labels":{OWNER:identity()}}])),
            result(false, &json!({})),
            result(true, &json!({})),
            result(true, &saved(false)),
            result(true, &json!({})),
        ]
    }
    for index in 0..8 {
        let dir = tempfile::tempdir().expect("directory");
        let mut replies = path();
        replies[index] = Err(Error::Dependency("injected command failure"));
        let probe = Arc::new(Probe::default());
        let backend =
            DockerBackend::with_dependencies(dir.path(), commands(replies), probe.clone());
        assert!(backend.reconcile(&spec()).await.is_err());
        assert!(!probe.0.load(Ordering::SeqCst));
    }
    for index in [0, 2, 3, 5, 6, 7] {
        let dir = tempfile::tempdir().expect("directory");
        let mut replies = path();
        replies[index] = result(false, &json!({}));
        let backend = DockerBackend::with_dependencies(
            dir.path(),
            commands(replies),
            Arc::new(Probe::default()),
        );
        assert!(backend.reconcile(&spec()).await.is_err());
    }
    for index in [3, 6] {
        let dir = tempfile::tempdir().expect("directory");
        let mut replies = path();
        replies[index] = Ok(CommandResult {
            success: true,
            stdout: b"bad JSON".to_vec(),
        });
        let backend = DockerBackend::with_dependencies(
            dir.path(),
            commands(replies),
            Arc::new(Probe::default()),
        );
        assert!(backend.reconcile(&spec()).await.is_err());
    }
    let dir = tempfile::tempdir().expect("directory");
    let mut backend =
        DockerBackend::with_dependencies(dir.path(), commands(path()), Arc::new(Probe::default()));
    backend.files.create = |_| Err(std::io::Error::other("disk failure"));
    assert!(backend.reconcile(&spec()).await.is_err());
    assert!(
        backend
            .ensure_managed("bad", "bad", 0, "bad")
            .await
            .is_err()
    );
    assert!(prepare_application_role(1, "bad", "bad").await.is_err());
}

#[tokio::test]
async fn database_probe_failure_is_not_ready_and_invalid_environment_paths_are_rejected() {
    #[derive(Debug)]
    struct BadProbe;
    impl DatabaseProbe for BadProbe {
        fn verify<'a>(&'a self, _: &'a DatabaseSpec) -> ResourceFuture<'a> {
            Box::pin(async { Err(Error::Invalid("database rejected")) })
        }
    }
    let dir = tempfile::tempdir().expect("directory");
    let backend =
        DockerBackend::with_dependencies(dir.path(), commands(vec![]), Arc::new(BadProbe));
    assert!(
        backend
            .reconcile(&DatabaseSpec::Existing { url: "url".into() })
            .await
            .is_err()
    );
    std::fs::create_dir(dir.path().join(format!("{}.env", identity())))
        .expect("directory at env path");
    assert!(
        backend
            .environment_file(&identity(), &"a".repeat(64))
            .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = dir.path().join(std::ffi::OsString::from_vec(vec![255]));
        assert!(path_string(&path).is_err());
    }
}

struct BrokenRead;
impl tokio::io::AsyncRead for BrokenRead {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("pipe failure")))
    }
}

#[tokio::test]
async fn command_transport_failures_never_return_success() {
    use std::io;
    let deadline = Duration::from_millis(20);
    assert!(
        complete(None::<tokio::io::Empty>, std::future::pending(), deadline)
            .await
            .is_err()
    );
    assert!(
        complete(Some(BrokenRead), std::future::pending(), deadline)
            .await
            .is_err()
    );
    assert!(
        complete(
            Some(tokio::io::empty()),
            std::future::ready(Err(io::Error::other("wait failure"))),
            deadline
        )
        .await
        .is_err()
    );
}

#[derive(Debug)]
struct RetryingProbe(std::sync::atomic::AtomicUsize);
impl DatabaseProbe for RetryingProbe {
    fn verify<'a>(&'a self, _: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(Error::Database(sqlx::Error::PoolClosed))
            } else {
                Ok(())
            }
        })
    }
}
#[tokio::test]
async fn a_starting_database_is_retried_before_becoming_ready() {
    let dir = tempfile::tempdir().expect("directory");
    let replies = vec![
        result(true, &json!({})),
        result(true, &json!({})),
        result(true, &json!([{"Labels":{OWNER:identity()}}])),
        result(true, &json!({})),
        result(true, &saved(true)),
    ];
    let probe = Arc::new(RetryingProbe(std::sync::atomic::AtomicUsize::new(0)));
    let backend = DockerBackend::with_dependencies(dir.path(), commands(replies), probe.clone());
    backend.reconcile(&spec()).await.expect("retried");
    assert_eq!(probe.0.load(Ordering::SeqCst), 2);
}

#[test]
fn docker_discovery_falls_back_to_path_and_prefers_a_real_installation() {
    let dir = tempfile::tempdir().expect("directory");
    let binary = dir.path().join("docker");
    assert_eq!(locate_docker(vec![binary.clone()]), PathBuf::from("docker"));
    std::fs::write(&binary, "executable").expect("file");
    assert_eq!(
        locate_docker(vec![dir.path().join("missing"), binary.clone()]),
        binary
    );
}

async fn complete(
    output: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
    wait: impl Future<Output = std::io::Result<std::process::ExitStatus>> + Send,
    deadline: Duration,
) -> Result<CommandResult, Error> {
    complete_command(
        output.map(|value| Box::new(value) as ProcessOutput),
        Box::pin(wait),
        deadline,
    )
    .await
}

async fn ensure<'a>(
    commands: &'a dyn DockerCommands,
    installation: impl Future<Output = Result<(), Error>> + Send + 'a,
) -> Result<(), Error> {
    ensure_available(commands, Box::pin(installation)).await
}
