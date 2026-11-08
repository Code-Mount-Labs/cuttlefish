#![allow(clippy::expect_used)]
use super::*;

#[test]
fn lifecycle_commands_are_explicit_and_stable() {
    for (action, name) in [
        (ServiceAction::Install, "install"),
        (ServiceAction::Start, "start"),
        (ServiceAction::Stop, "stop"),
        (ServiceAction::Restart, "restart"),
        (ServiceAction::Status, "status"),
        (ServiceAction::Uninstall, "uninstall"),
    ] {
        assert_eq!(action.name(), name);
        assert!(!format!("{action:?}").is_empty());
    }
}
#[tokio::test]
async fn platform_and_input_errors_do_not_install_services() {
    let root = tempfile::tempdir().expect("root");
    let exe = std::env::current_exe().expect("exe");
    let address = "127.0.0.1:8080".parse().expect("address");
    let home = Some(root.path().as_os_str());
    for (os, listen, home) in [
        ("linux", address, home),
        ("macos", "127.0.0.1:0".parse().expect("zero port"), home),
        ("macos", address, None),
    ] {
        assert!(
            apply(
                ServiceAction::Install,
                root.path(),
                &exe,
                listen,
                home,
                os,
                "exit 0"
            )
            .await
            .is_err()
        );
    }
    assert!(
        apply(
            ServiceAction::Install,
            &root.path().join("missing"),
            &exe,
            address,
            home,
            "macos",
            "exit 0"
        )
        .await
        .is_err()
    );
    assert!(
        apply(
            ServiceAction::Install,
            root.path(),
            &root.path().join("missing"),
            address,
            home,
            "macos",
            "exit 0"
        )
        .await
        .is_err()
    );
    // Exercise the production platform dispatch without a valid state directory.
    assert!(
        manage(
            ServiceAction::Status,
            &root.path().join("missing"),
            &exe,
            address,
            home
        )
        .await
        .is_err()
    );
}
#[cfg(unix)]
#[tokio::test]
async fn service_adapter_passes_exact_paths_without_shell_interpolation() {
    let root = tempfile::Builder::new()
        .prefix("service & ' test ")
        .tempdir()
        .expect("root");
    let exe = std::env::current_exe().expect("exe");
    let address = "127.0.0.1:8080".parse().expect("address");
    let canonical = std::fs::canonicalize(root.path()).expect("canonical root");
    let home = Some(canonical.as_os_str());
    apply(ServiceAction::Install,root.path(),&exe,address,home,"macos",
        "set -eu; [ \"$1\" = install ]; [ -d \"$2\" ]; [ -f \"$3\" ]; [ \"$4\" = '127.0.0.1:8080' ]; [ \"$5\" = 127.0.0.1 ]; [ \"$6\" = 8080 ]; [ \"$7\" = \"$2\" ]").await.expect("valid adapter");
    assert!(
        apply(
            ServiceAction::Status,
            root.path(),
            &exe,
            address,
            home,
            "macos",
            "exit 1"
        )
        .await
        .is_err()
    );
}

#[test]
fn saved_service_listener_is_specific_to_state_and_validated() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().expect("temp");
    let state = temp.path().join("state");
    std::fs::create_dir(&state).expect("state");
    assert!(
        address(&state, Some(temp.path()), "linux")
            .expect("linux")
            .is_none()
    );
    assert!(address(&state, None, "macos").expect("no home").is_none());
    assert!(
        address(&state, Some(temp.path()), "macos")
            .expect("absent")
            .is_none()
    );
    assert!(address(&state.join("missing"), Some(temp.path()), "macos").is_err());
    let canonical = std::fs::canonicalize(&state).expect("canonical");
    let hash = format!(
        "{:x}",
        Sha256::digest(canonical.as_os_str().as_encoded_bytes())
    );
    let folder = temp.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&folder).expect("folder");
    let path = folder.join(format!("com.cuttlefish.app.{}.plist", &hash[..32]));
    for content in [
        "unrecognized",
        "<!-- Cuttlefish listener: bad -->",
        "<!-- Cuttlefish listener: 127.0.0.1:8123 -->",
    ] {
        std::fs::write(&path, content).expect("plist");
        assert_eq!(
            address(&state, Some(temp.path()), "macos").is_ok(),
            content.ends_with("8123 -->")
        );
    }
    std::fs::remove_file(&path).expect("remove");
    std::fs::create_dir(&path).expect("unreadable");
    assert!(address(&state, Some(temp.path()), "macos").is_err());
}

#[tokio::test]
async fn empty_executable_is_rejected_before_service_invocation() {
    let dir = tempfile::tempdir().expect("directory");
    assert!(
        apply(
            ServiceAction::Install,
            dir.path(),
            Path::new(""),
            "127.0.0.1:8123".parse().expect("address"),
            Some(dir.path().as_os_str()),
            "macos",
            "exit 0"
        )
        .await
        .is_err()
    );
}
