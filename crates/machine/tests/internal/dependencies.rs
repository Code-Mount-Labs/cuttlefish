#![allow(clippy::expect_used)]
use super::*;
use std::{
    io,
    pin::Pin,
    process::ExitStatus,
    task::{Context, Poll},
};
#[derive(Debug)]
struct Refuse;
impl tokio::io::AsyncWrite for Refuse {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("injected pipe failure")))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
fn status(code: u32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(i32::try_from(code).expect("status"))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code)
    }
}
#[tokio::test]
async fn installer_reports_process_pipe_wait_exit_and_deadline_failures() {
    let deadline = Duration::from_millis(20);
    assert!(
        run_installer(
            Command::new("/missing/cuttlefish-installer"),
            "",
            false,
            deadline
        )
        .await
        .is_err()
    );
    assert!(
        complete_installer(
            None::<tokio::io::Sink>,
            std::future::ready(Ok(status(0))),
            "",
            deadline
        )
        .await
        .is_err()
    );
    assert!(
        complete_installer(
            Some(Refuse),
            std::future::ready(Ok(status(0))),
            "script",
            deadline
        )
        .await
        .is_err()
    );
    assert!(
        complete_installer(
            Some(tokio::io::sink()),
            std::future::ready(Err(io::Error::other("wait failed"))),
            "",
            deadline
        )
        .await
        .is_err()
    );
    assert!(
        complete_installer(
            Some(tokio::io::sink()),
            std::future::ready(Ok(status(256))),
            "",
            deadline
        )
        .await
        .is_err()
    );
    assert!(
        complete_installer(
            Some(tokio::io::sink()),
            std::future::pending(),
            "",
            deadline
        )
        .await
        .is_err()
    );
    complete_installer(
        Some(tokio::io::sink()),
        std::future::ready(Ok(status(0))),
        "",
        deadline,
    )
    .await
    .expect("success");
}
