#![allow(clippy::expect_used)]
use super::*;
fn noop(_: &Manager, _: std::net::SocketAddr) -> Work<'_> {
    Box::pin(async { Ok(()) })
}
#[tokio::test]
async fn native_installer_uses_exact_arguments_and_reports_failure() {
    use super::Operations;
    let (temp, manager) = super::super::tests::fixture();
    let native = Native {
        script: "[ \"$CUTTLEFISH_VERSION\" = 'v0.2.0-alpha.7' ] && [ \"$CUTTLEFISH_NO_SETUP\" = 1 ] && [ -d \"$CUTTLEFISH_INSTALL_DIR\" ]",
        restart: noop,
    };
    native
        .install(temp.path(), "v0.2.0-alpha.7")
        .await
        .expect("verified invocation");
    assert!(native.install(temp.path(), "incorrect").await.is_err());
    let default = Native::default();
    assert!(
        default
            .script
            .contains("Release checksum verification failed")
    );
    // The production service boundary receives an invalid listener and must fail
    // before ever invoking launchctl or touching the account's services.
    assert!(
        restart_service(&manager, "127.0.0.1:0".parse().expect("address"))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn restart_health_check_requires_successful_matching_version() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_temp, manager) = super::super::tests::fixture();
    for (response, succeeds) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 27\r\n\r\n{\"version\":\"0.2.0-alpha.7\"}",
            true,
        ),
        ("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", false),
        ("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad", false),
        (
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
            false,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response");
        });
        let native = Native {
            script: "",
            restart: noop,
        };
        assert_eq!(
            native
                .restart(&manager, address, "v0.2.0-alpha.7")
                .await
                .is_ok(),
            succeeds
        );
        task.await.expect("server");
    }
    assert!(verify("bad:", "v0.2.0-alpha.7").await.is_err());
}

#[tokio::test]
async fn native_restart_and_transport_failures_do_not_report_success() {
    assert!(
        verify_with(
            "http://127.0.0.1",
            "v0.2.0",
            reqwest::Client::builder().user_agent("invalid\nheader")
        )
        .await
        .is_err()
    );
    let (_temp, manager) = super::super::tests::fixture();
    let native = Native::default();
    assert!(
        native
            .restart(&manager, "127.0.0.1:0".parse().expect("address"), "v0.2.0")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn progress_write_and_lock_failures_never_advance_a_job() {
    let (_temp, manager) = super::super::tests::fixture();
    let mut journal = manager.store().expect("store");
    let operations = super::super::tests::Fake::default();
    let root = manager.root().expect("root");
    std::fs::create_dir(root.join("update.lock")).expect("block lock");
    let mut pending = super::super::tests::job(Phase::Queued);
    assert!(
        execute(&manager, &operations, &journal, &mut pending)
            .await
            .is_err()
    );
    std::fs::remove_dir(root.join("update.lock")).expect("unblock");
    journal.files.sync = |_| Err(std::io::Error::other("disk failed"));
    assert!(
        execute(&manager, &operations, &journal, &mut pending)
            .await
            .is_err()
    );
    super::super::tests::service(&manager);
    pending.restart_only = true;
    assert!(
        execute(&manager, &operations, &journal, &mut pending)
            .await
            .is_err()
    );
    journal
        .save_job(pending.clone())
        .expect_err("write stays failed");
    let journal = manager.store().expect("store");
    journal.save_job(pending).expect("save");
    let guard = journal.lock().expect("lock");
    let damage = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::remove_file(journal.directory.join("worker.lock")).expect("unlink lock");
        std::fs::create_dir(journal.directory.join("worker.lock")).expect("replace lock");
        drop(guard);
    };
    let (result, ()) = tokio::join!(run(&manager, &operations), damage);
    assert!(result.is_err());
}
#[tokio::test]
async fn failed_failure_record_is_not_reported_as_success() {
    let (_temp, manager) = super::super::tests::fixture();
    super::super::tests::save(&manager, super::super::tests::job(Phase::Queued));
    let mut operations = super::super::tests::Fake::default();
    operations.lose_journal = true;
    assert!(run(&manager, &operations).await.is_err());
}
