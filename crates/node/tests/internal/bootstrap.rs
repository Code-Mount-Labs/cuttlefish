#![allow(clippy::expect_used)]
use super::*;

#[test]
fn runtime_bootstrap_failure_is_returned() {
    let args = Cli::try_parse_from(["node", "stop", "--state-dir", "."]).expect("arguments");
    let result = execute(args, || {
        Err(std::io::Error::other("runtime initialization failed"))
    });
    assert!(matches!(result, Err(NodeError::Io(_))));
    let temp = tempfile::tempdir().expect("directory");
    let args = Cli::try_parse_from([
        "node",
        "stop",
        "--state-dir",
        temp.path().to_str().expect("path"),
    ])
    .expect("arguments");
    assert!(
        execute(args, || tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build())
        .is_err()
    );
}
