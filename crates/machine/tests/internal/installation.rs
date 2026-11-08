#![allow(clippy::expect_used)]
use super::*;
#[test]
fn managed_layout_resolves_stable_entrypoint_and_rejects_unmanaged_paths() {
    let temp = tempfile::tempdir().expect("temp");
    let exe = temp.path().join("releases/v1/cuttlefish");
    std::fs::create_dir_all(exe.parent().expect("parent")).expect("folder");
    std::fs::write(&exe, b"binary").expect("exe");
    assert!(root(&exe).is_err());
    std::fs::write(temp.path().join(".cuttlefish-layout"), b"unknown").expect("marker");
    assert!(root(&exe).is_err());
    std::fs::write(
        temp.path().join(".cuttlefish-layout"),
        b"cuttlefish-release-layout-v1\n",
    )
    .expect("marker");
    assert_eq!(
        root(&exe).expect("root"),
        std::fs::canonicalize(temp.path()).expect("canonical")
    );
    assert!(stable(&exe).ends_with("bin/cuttlefish"));
    assert_eq!(
        stable(Path::new("/missing-cuttlefish")),
        Path::new("/missing-cuttlefish")
    );
    assert!(root(Path::new("/")).is_err());
    assert!(root(temp.path()).is_err());
}
