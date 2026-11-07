//! Bounded framing, partial reads, errors, and local JSON control operations.
#![allow(clippy::expect_used)]

use super::common;

use std::{
    io,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use clap::Parser;
use common::{Fixture, id, identity, start};
use orchestrator_node::{
    cli::{self, Cli, Request, Response},
    model::{LaunchSpec, WorkspaceMount},
    process::{Launcher, LocalLauncher, ManagedProcess},
    runtime::NodeRuntime,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};

#[derive(Debug)]
pub(super) struct Noop;
impl Launcher for Noop {
    type Process = Self;
    fn spawn(&mut self, _: &Path, _: &LaunchSpec) -> io::Result<Self> {
        Ok(Self)
    }
}
impl ManagedProcess for Noop {
    fn try_wait(&mut self) -> io::Result<Option<bool>> {
        Ok(None)
    }
}

fn line(request: &Request) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(request).expect("request");
    bytes.push(b'\n');
    bytes
}

#[tokio::test]
async fn every_control_operation_returns_typed_responses() {
    let fixture = Fixture::new().await;
    fixture.completed().await;
    let event = fixture.journal.pending_events().await.expect("events")[0].sequence;
    let mut runtime = NodeRuntime::open(fixture.temp.path(), identity(), Noop)
        .await
        .expect("runtime");
    let requests = [
        Request::Mount {
            mount: WorkspaceMount {
                workspace_id: id(5),
                path: fixture.temp.path().to_path_buf(),
            },
        },
        Request::Submit {
            command: Box::new(start()),
        },
        Request::Session { session_id: id(4) },
        Request::Command { command_id: id(6) },
        Request::Events,
        Request::Acknowledge { sequence: event },
        Request::Command {
            command_id: id(999),
        },
        Request::Session {
            session_id: id(999),
        },
        Request::Shutdown,
    ];
    let input: Vec<u8> = requests.iter().flat_map(line).collect();
    let mut output = Vec::new();
    serve(&mut runtime, input.as_slice(), &mut output)
        .await
        .expect("serve");
    let responses: Vec<Response> = String::from_utf8(output)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("response"))
        .collect();
    assert!(matches!(responses[0], Response::Ready { .. }));
    assert!(matches!(responses[1], Response::Mounted));
    assert!(matches!(responses[2], Response::Receipt { .. }));
    assert!(matches!(
        responses[3],
        Response::Session { record: Some(_) }
    ));
    assert!(matches!(
        responses[4],
        Response::Command { record: Some(_) }
    ));
    assert!(matches!(responses[5], Response::Events { .. }));
    assert!(matches!(
        responses[6],
        Response::Acknowledged { found: true }
    ));
    assert!(matches!(responses[7], Response::Command { record: None }));
    assert!(matches!(responses[8], Response::Session { record: None }));
    assert!(matches!(responses[9], Response::Bye));
    runtime.journal().close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn framing_handles_errors_eof_and_rejects_oversized_input() {
    let temp = tempfile::tempdir().expect("directory");
    let mut runtime = NodeRuntime::open(temp.path(), identity(), Noop)
        .await
        .expect("runtime");
    let mut input = b"not json\n".to_vec();
    input.extend(line(&Request::Submit {
        command: Box::new(start()),
    })); // Unmounted workspace.
    input.extend(b"{\"type\":\"events\"}"); // Valid last frame without newline.
    let mut output = Vec::new();
    serve(&mut runtime, input.as_slice(), &mut output)
        .await
        .expect("EOF is graceful");
    let text = String::from_utf8(output).expect("utf8");
    assert_eq!(text.matches("\"type\":\"error\"").count(), 2);
    assert!(text.contains("\"type\":\"events\""));
    let oversized = vec![b'x'; cli::MAX_FRAME_BYTES + 1];
    assert!(
        serve(&mut runtime, oversized.as_slice(), Vec::new())
            .await
            .is_err()
    );
    runtime.journal().close().await;
}

#[tokio::test]
async fn a_supervision_tick_does_not_discard_a_partial_json_frame() {
    let temp = tempfile::tempdir().expect("directory");
    let mut runtime = NodeRuntime::open(temp.path(), identity(), Noop)
        .await
        .expect("runtime");
    let (mut writer, reader) = tokio::io::duplex(128);
    let writing = tokio::spawn(async move {
        writer
            .write_all(b"{\"type\":")
            .await
            .expect("partial frame");
        tokio::time::sleep(Duration::from_millis(250)).await;
        writer
            .write_all(b"\"shutdown\"}\n")
            .await
            .expect("rest of frame");
    });
    let mut output = Vec::new();
    serve(&mut runtime, BufReader::new(reader), &mut output)
        .await
        .expect("complete frame");
    writing.await.expect("writer");
    assert!(
        String::from_utf8(output)
            .expect("utf8")
            .contains("\"type\":\"bye\"")
    );
    runtime.journal().close().await;
}

struct BrokenReader;
impl AsyncRead for BrokenReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("read failed")))
    }
}

pub(super) struct BrokenWriter {
    pub(super) fail_write: bool,
}
impl AsyncWrite for BrokenWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(if self.fail_write {
            Err(io::Error::other("write failed"))
        } else {
            Ok(bytes.len())
        })
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("flush failed")))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn stream_and_background_failures_are_reported() {
    let temp = tempfile::tempdir().expect("directory");
    let mut runtime = NodeRuntime::open(temp.path(), identity(), Noop)
        .await
        .expect("runtime");
    assert!(
        serve(&mut runtime, BufReader::new(BrokenReader), Vec::new())
            .await
            .is_err()
    );
    for fail_write in [true, false] {
        assert!(
            serve(&mut runtime, &b""[..], BrokenWriter { fail_write })
                .await
                .is_err()
        );
    }
    runtime.journal().close().await;
    let (_keep_open, reader) = tokio::io::duplex(1);
    assert!(
        serve(&mut runtime, BufReader::new(reader), Vec::new())
            .await
            .is_err()
    );
}

#[test]
fn command_line_rejects_missing_or_invalid_arguments() {
    for args in [
        vec!["node"],
        vec!["node", "run"],
        vec!["node", "unknown"],
        vec![
            "node",
            "run",
            "--state-dir",
            ".",
            "--group",
            "bad",
            "--node",
            "bad",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[tokio::test]
async fn launcher_rejects_changed_workspaces_and_reports_os_errors() {
    let fixture = Fixture::new().await;
    let accepted = fixture
        .journal
        .accept(&start(), common::instance())
        .await
        .expect("accept");
    let original = accepted.launch.expect("launch");
    let mut launcher = LocalLauncher::new(fixture.temp.path().join("missing-executable"));
    let mut missing = original.clone();
    missing.directory = fixture.temp.path().join("absent");
    assert!(launcher.spawn(fixture.journal.path(), &missing).is_err());
    let mut changed = original.clone();
    changed.directory = fixture.journal.path().canonicalize().expect("file");
    assert!(launcher.spawn(fixture.journal.path(), &changed).is_err());
    changed.directory = original.directory.join(".");
    assert!(launcher.spawn(fixture.journal.path(), &changed).is_err());
    assert!(
        launcher
            .spawn(&fixture.temp.path().join("absent/journal"), &original)
            .is_err()
    );
    assert!(launcher.spawn(fixture.journal.path(), &original).is_err());
    fixture.journal.close().await;
}

// Exercise the same stream implementation for normal I/O and injected faults.
pub(super) async fn serve<R: tokio::io::AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    runtime: &mut NodeRuntime<Noop>,
    input: R,
    output: W,
) -> Result<(), orchestrator_node::NodeError> {
    let input: Box<dyn tokio::io::AsyncBufRead + Unpin + '_> = Box::new(input);
    let output: Box<dyn AsyncWrite + Unpin + '_> = Box::new(output);
    cli::serve(runtime, input, output).await
}
