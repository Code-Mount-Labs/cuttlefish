#![allow(clippy::expect_used)]
use super::*;
use uuid::Uuid;
fn lease() -> RelayLease {
    RelayLease {
        installation_id: InstallationId::random(),
        epoch: Uuid::new_v4(),
        public_url: "https://relay.test/i/test".into(),
        issuer: "https://platform.test".into(),
    }
}
#[cfg(unix)]
#[tokio::test]
async fn native_adapter_writes_private_immutable_configuration_and_propagates_failures() {
    let dir = tempfile::tempdir().expect("directory");
    let mut connector = NativeConnector::new(dir.path());
    connector.script = "exit 0";
    let lease = lease();
    connector
        .start(&lease, "test-credential", "http://127.0.0.1:8080")
        .await
        .expect("start");
    connector
        .start(&lease, "test-credential", "http://127.0.0.1:8080")
        .await
        .expect("retry");
    connector.stop(lease.installation_id).await.expect("stop");
    assert!(
        connector
            .start(&lease, "changed-credential", "http://127.0.0.1:8080")
            .await
            .is_err()
    );
    connector.script = "exit 1";
    assert!(connector.stop(lease.installation_id).await.is_err());
    assert!(
        connector
            .start(&lease, "test-credential", "http://127.0.0.1:8080")
            .await
            .is_err()
    );
    let root = connector.root().expect("root");
    let label = connector
        .plan(&lease, "test-credential", "http://127.0.0.1:8080", "macos")
        .expect("plan");
    let config = std::fs::read_to_string(root.join(format!("{label}.json"))).expect("config");
    assert!(config.contains("\"useSystemCa\":true"));
    assert!(
        !std::fs::read_to_string(root.join(format!("{label}.plist")))
            .expect("service")
            .contains("test-credential")
    );
    assert!(connector.plan(&lease, "x", "invalid", "linux").is_err());
    assert!(
        connector
            .plan(&lease, "x", "http://192.0.2.1", "linux")
            .is_err()
    );
    assert!(
        connector
            .plan(
                &RelayLease {
                    public_url: "bad".into(),
                    ..lease.clone()
                },
                "x",
                "http://127.0.0.1",
                "linux"
            )
            .is_err()
    );
    assert!(
        connector
            .plan(
                &RelayLease {
                    public_url: "http://127.0.0.1".into(),
                    ..lease
                },
                "x",
                "http://127.0.0.1",
                "linux"
            )
            .is_err()
    );
}
#[test]
fn service_manifests_escape_paths_and_reject_unsupported_platforms() {
    let binary = Path::new("/test/a & <b> \"'%.bin");
    let config = Path::new("/test/c\\d");
    let (_, mac) = service_unit("macos", "label", binary, config).expect("mac");
    assert!(mac.contains("&amp;"));
    assert!(mac.contains("&lt;"));
    assert!(mac.contains("&gt;"));
    assert!(mac.contains("&quot;"));
    assert!(mac.contains("&apos;"));
    let (_, linux) = service_unit("linux", "label", binary, config).expect("linux");
    assert!(linux.contains("%%"));
    assert!(linux.contains("\\\\"));
    assert!(service_unit("windows", "label", binary, config).is_err());
    assert!(service_unit("linux", "label", Path::new("/bad\n"), config).is_err());
    let dir = tempfile::tempdir().expect("directory");
    assert!(
        NativeConnector::new(dir.path())
            .plan(&lease(), "x", "http://127.0.0.1", "windows")
            .is_err()
    );
    let target = dir.path().join("file");
    std::fs::create_dir(&target).expect("directory");
    assert!(write_once(dir.path(), &target, b"x").is_err());
    assert!(
        NativeConnector::new(&target.join("missing/file"))
            .root()
            .is_ok()
    );
    std::fs::write(dir.path().join("ordinary"), b"x").expect("file");
    assert!(
        NativeConnector::new(&dir.path().join("ordinary"))
            .root()
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn inaccessible_paths_and_manifest_conflicts_do_not_start_a_service() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().expect("directory");
    let lease = lease();
    let mut connector = NativeConnector::new(dir.path());
    connector.script = "exit 0";
    assert!(
        connector
            .plan(&lease, "x", "mailto:invalid", "linux")
            .is_err()
    );
    assert!(
        connector
            .plan(&lease, "x", "http://127.0.0.1", "windows")
            .is_err()
    );
    let invalid = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
    assert!(service_unit("linux", "label", invalid, Path::new("valid")).is_err());
    assert!(service_unit("linux", "label", Path::new("valid"), invalid).is_err());
    let label = connector
        .plan(&lease, "x", "http://127.0.0.1", "macos")
        .expect("plan");
    let root = connector.root().expect("root");
    std::fs::write(root.join(format!("{label}.plist")), "different").expect("conflict");
    assert!(
        connector
            .plan(&lease, "x", "http://127.0.0.1", "macos")
            .is_err()
    );
    assert!(write_once(dir.path(), &dir.path().join("absent/file"), b"x").is_err());
    let file = dir.path().join("blocker");
    std::fs::write(&file, "x").expect("file");
    let connector = NativeConnector::new(&file);
    assert!(connector.stop(lease.installation_id).await.is_err());
    assert!(
        connector
            .plan(&lease, "x", "http://127.0.0.1", "linux")
            .is_err()
    );
}
