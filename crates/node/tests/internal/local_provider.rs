#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, instance, start};
use orchestrator_protocol::execution::ProviderWorkspace;
fn config(f: &Fixture, provider: LocalProvider) -> ExecutionConfig {
    let directory = f
        .journal
        .path()
        .parent()
        .expect("parent")
        .canonicalize()
        .expect("canonical workspace");
    let instructions = directory.join("specialist.md");
    std::fs::write(&instructions, "Be precise.").expect("prompt");
    ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Test".into(),
            provider,
            allow_edits: false,
        },
        executable: "/bin/cat".into(),
        directory,
        instructions,
    }
}
async fn agent(provider: LocalProvider) -> (Fixture, LocalAgent) {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    let adapter = LocalAgent::new(
        f.journal.clone(),
        instance().to_string(),
        config(&f, provider),
    );
    (f, adapter)
}
fn wire(events: &[Value]) -> Wire {
    let mut bytes = Vec::new();
    for value in events {
        serde_json::to_writer(&mut bytes, value).expect("json");
        bytes.push(b'\n');
    }
    Wire {
        input: Box::new(Vec::<u8>::new()),
        output: Box::new(std::io::Cursor::new(bytes)),
        remaining: 32 * 1024 * 1024,
        pending: VecDeque::new(),
    }
}
fn events() -> Vec<Value> {
    vec![
        json!({"id":1,"result":{}}),
        json!({"id":2,"result":{"thread":{"id":"session-one"}}}),
        json!({"method":"item/completed","params":{"threadId":"session-one","turnId":"turn-one","item":{"type":"agentMessage","text":"Done"}}}),
        json!({"id":3,"result":{"turn":{"id":"turn-one"}}}),
        json!({"method":"turn/completed","params":{"threadId":"session-one","turn":{"id":"turn-one","status":"completed"}}}),
    ]
}
#[tokio::test]
async fn codex_binds_before_effects_and_preserves_early_notifications_and_resume() {
    let (f, mut a) = agent(LocalProvider::Codex).await;
    assert!(a.failure_is_uncertain());
    let mut w = wire(&events());
    assert_eq!(
        a.codex(&start(), "hello", None, &mut w)
            .await
            .expect("turn"),
        "Done"
    );
    let binding = f
        .journal
        .provider_binding(id(4))
        .await
        .expect("binding")
        .expect("saved");
    assert_eq!(binding.provider_session_id.as_str(), "session-one");
    a.config.workspace.allow_edits = true;
    let mut e = events();
    e.insert(
        3,
        json!({"method":"item/completed","params":{"threadId":"other"}}),
    );
    e.insert(
        4,
        json!({"method":"ignored","params":{"threadId":"session-one"}}),
    );
    let mut w = wire(&e);
    let (capture, mut reader) = tokio::io::duplex(65_536);
    w.input = Box::new(capture);
    assert_eq!(
        a.codex(&start(), "again", Some(&binding), &mut w)
            .await
            .expect("resume"),
        "Done"
    );
    drop(w);
    let mut sent = String::new();
    reader
        .read_to_string(&mut sent)
        .await
        .expect("captured request");
    assert!(sent.contains("thread/resume"));
    assert!(sent.contains("workspaceWrite"));
    assert_eq!(
        a.execute(&crate::test_support::cancel(9))
            .await
            .expect("cancel")
            .kind,
        AgentEventKind::Cancelled
    );
}
#[tokio::test]
async fn codex_rejects_missing_identity_failed_turns_and_invalid_prompts() {
    let (f, a) = agent(LocalProvider::Codex).await;
    for (index, value) in [
        (1, json!({"id":2,"result":{}})),
        (2, json!({"id":2,"result":{"thread":{"id":""}}})),
        (3, json!({"id":3,"result":{}})),
        (
            4,
            json!({"method":"turn/completed","params":{"threadId":"session-one","turn":{"id":"turn-one","status":"failed"}}}),
        ),
    ] {
        let mut e = events();
        if index == 2 {
            e[1] = value;
        } else {
            e[index] = value;
        }
        assert!(
            a.codex(&start(), "hello", None, &mut wire(&e))
                .await
                .is_err()
        );
    }
    for prompt in [String::new(), "x".repeat(65_537)] {
        std::fs::write(&a.config.instructions, prompt).expect("prompt");
        assert!(
            a.codex(&start(), "hello", None, &mut wire(&events()))
                .await
                .is_err()
        );
    }
    std::fs::remove_file(&a.config.instructions).expect("remove");
    assert!(
        a.codex(&start(), "hello", None, &mut wire(&events()))
            .await
            .is_err()
    );
    f.journal.close().await;
}
#[tokio::test]
async fn claude_requires_matching_session_and_successful_terminal_response() {
    let (f, a) = agent(LocalProvider::Claude).await;
    let id = start().target.session_id.to_string();
    a.bind(&start(), &id).await.expect("bind");
    let terminal = json!({"session_id":id,"type":"result","subtype":"success","is_error":false,"result":"Done"});
    let mut w = wire(&[json!({"type":"system"}), terminal.clone()]);
    assert_eq!(a.claude(&start(), &mut w).await.expect("done"), "Done");
    for (field, value) in [
        ("session_id", Value::Null),
        ("session_id", json!("wrong")),
        ("is_error", json!(true)),
        ("subtype", json!("error")),
        ("result", Value::Null),
    ] {
        let mut event = terminal.clone();
        event[field] = value;
        assert!(a.claude(&start(), &mut wire(&[event])).await.is_err());
    }
    assert!(
        a.run(&crate::test_support::send(10), "follow-up")
            .await
            .is_err()
    ); // cat rejects CLI flags
    f.journal.close().await;
}
#[tokio::test]
async fn bounded_wire_rejects_bad_json_oversize_streams_and_interactive_requests() {
    for raw in [
        vec![],
        b"not json\n".to_vec(),
        vec![b'x'; MAX_PROVIDER_FRAME_BYTES + 1],
        b"{\"id\":1,\"method\":\"approval\"}\n".to_vec(),
    ] {
        let mut w = wire(&[]);
        w.output = Box::new(std::io::Cursor::new(raw));
        assert!(w.read().await.is_err());
    }
    let mut w = wire(&[json!({})]);
    w.remaining = 1;
    assert!(w.read().await.is_err());
    let mut w = wire(&[json!({"id":1,"error":{}})]);
    assert!(w.response(1).await.is_err());
    let mut w = wire(&vec![json!({"method":"notification"}); 1025]);
    assert!(w.response(1).await.is_err());
    let (f, a) = agent(LocalProvider::Codex).await;
    assert!(
        a.run(&crate::test_support::send(10), "follow-up")
            .await
            .is_err()
    );
    assert!(a.run(&start(), "hello").await.is_err());
    f.journal.close().await;
}

#[cfg(unix)]
fn script(a: &mut LocalAgent, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    a.config.executable = a.config.directory.join("fixture-cli");
    std::fs::write(&a.config.executable, format!("#!/bin/sh\n{body}\n")).expect("script");
    std::fs::set_permissions(&a.config.executable, std::fs::Permissions::from_mode(0o700))
        .expect("mode");
}
#[cfg(unix)]
#[tokio::test]
async fn process_failures_never_become_successful_or_replayed_turns() {
    let (_f, mut a) = agent(LocalProvider::Claude).await;
    let terminal = json!({"type":"result","session_id":start().target.session_id,"is_error":false,"subtype":"success","result":"Done"});
    script(
        &mut a,
        &format!("cat >/dev/null\nprintf '%s\\n' '{terminal}'\nexit 1"),
    );
    assert!(a.run(&start(), "hello").await.is_err());
    script(&mut a, "sleep 1");
    assert!(
        a.run_with_timeout(&start(), "hello", Duration::from_millis(10))
            .await
            .is_err()
    );
    let mut blank = terminal;
    blank["result"] = json!("");
    script(&mut a, &format!("cat >/dev/null\nprintf '%s\\n' '{blank}'"));
    assert!(a.execute(&start()).await.is_err());
    a.config.executable = a.config.directory.join("missing");
    assert!(a.run(&start(), "hello").await.is_err());
    for (stdin, stdout) in [
        (Stdio::null(), Stdio::null()),
        (Stdio::piped(), Stdio::null()),
    ] {
        let mut child = Command::new("/usr/bin/true")
            .stdin(stdin)
            .stdout(stdout)
            .spawn()
            .expect("child");
        assert!(transport(&mut child).is_err());
        child.wait().await.expect("exit");
    }
    a.journal.close().await;
    assert!(a.run(&start(), "hello").await.is_err());
}
#[derive(Default)]
struct FailingWriter {
    writes: usize,
    fail_at: usize,
    flush_error: bool,
}
impl AsyncWrite for FailingWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.writes += 1;
        std::task::Poll::Ready(if self.writes == self.fail_at {
            Err(std::io::Error::other("write failed"))
        } else {
            Ok(buffer.len())
        })
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(if self.flush_error {
            Err(std::io::Error::other("flush failed"))
        } else {
            Ok(())
        })
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn transport_disconnects_at_each_rpc_boundary_are_explicit() {
    let (_f, a) = agent(LocalProvider::Codex).await;
    for fail_at in 1..=4 {
        let w = wire(&events());
        let mut w = Wire {
            input: Box::new(FailingWriter {
                fail_at,
                ..FailingWriter::default()
            }),
            output: w.output,
            remaining: w.remaining,
            pending: w.pending,
        };
        assert!(a.codex(&start(), "hello", None, &mut w).await.is_err());
    }
    let w = wire(&events());
    let mut w = Wire {
        input: Box::new(FailingWriter {
            flush_error: true,
            ..FailingWriter::default()
        }),
        output: w.output,
        remaining: w.remaining,
        pending: w.pending,
    };
    assert!(a.codex(&start(), "hello", None, &mut w).await.is_err());
    for end in [0, 1, 2, 4] {
        assert!(
            a.codex(&start(), "hello", None, &mut wire(&events()[..end]))
                .await
                .is_err()
        );
    }
    assert!(a.claude(&start(), &mut wire(&[])).await.is_err());
}

#[tokio::test]
async fn native_media_frames_can_exceed_text_limits_without_losing_notifications() {
    let mut w = wire(&[
        json!({"method":"item/completed","params":{"item":{"type":"imageGeneration","data":"a".repeat(2*1024*1024)}}}),
        json!({"id":1,"result":{}}),
    ]);
    w.response(1).await.expect("response after media");
    assert_eq!(
        w.read().await.expect("retained media")["params"]["item"]["data"]
            .as_str()
            .expect("data")
            .len(),
        2 * 1024 * 1024
    );
}

#[tokio::test]
async fn provider_preconditions_reject_changed_workspace_bad_input_and_invalid_encoding() {
    let (_f, mut a) = agent(LocalProvider::Codex).await;
    let directory = a.config.directory.clone();
    a.config.directory = directory.join("missing");
    assert!(a.run(&start(), "test").await.is_err());
    a.config.directory = directory;
    let mut command = start();
    if let AgentAction::Start { message, .. } = &mut command.action {
        message.attachments = vec![orchestrator_protocol::attachments::Attachment {
            id: orchestrator_protocol::ids::AttachmentId::random(),
            workspace_id: id(5),
            name: "../bad".into(),
            media_type: "text/plain".into(),
            size: 0,
            sha256: "a".repeat(64),
        }];
    }
    assert!(a.run(&command, "test").await.is_err());
    std::fs::write(&a.config.instructions, [255]).expect("invalid encoding");
    assert!(
        a.codex(&start(), "test", None, &mut wire(&events()))
            .await
            .is_err()
    );
    let (_f, mut a) = agent(LocalProvider::Claude).await;
    script(&mut a, "exit 0");
    assert!(a.run(&start(), &"x".repeat(1_048_576)).await.is_err());
}

#[tokio::test]
async fn successful_provider_reply_is_not_published_when_artifact_storage_fails() {
    let (_f, mut a) = agent(LocalProvider::Claude).await;
    a.config.workspace.allow_edits = true;
    let terminal = json!({"type":"result","session_id":start().target.session_id,"is_error":false,"subtype":"success","result":"Done"});
    let target = a
        .config
        .directory
        .join("artifacts")
        .join(start().command_id.to_string());
    std::fs::create_dir_all(target.join("nested")).expect("invalid nested output");
    script(
        &mut a,
        &format!("cat >/dev/null\nprintf '%s\\n' '{terminal}'"),
    );
    assert!(a.execute(&start()).await.is_err());
}

#[tokio::test]
async fn process_setup_exit_and_reader_failures_are_propagated() {
    for (input, output) in [
        (Stdio::null(), Stdio::null()),
        (Stdio::piped(), Stdio::null()),
    ] {
        let mut process = Command::new("/usr/bin/true");
        process.stdin(input).stdout(output).kill_on_drop(true);
        assert!(spawn(&mut process).is_err());
    }
    assert!(successful_exit(Err(std::io::Error::other("lost process handle"))).is_err());
    let (_f, mut a) = agent(LocalProvider::Claude).await;
    a.instance = "wrong-worker".into();
    assert!(a.run(&start(), "test").await.is_err());
    let mut w = Wire {
        input: Box::new(Vec::<u8>::new()),
        output: Box::new(BufReader::new(FailingReader)),
        remaining: 1024,
        pending: VecDeque::new(),
    };
    assert!(w.read().await.is_err());
}
struct FailingReader;
impl tokio::io::AsyncRead for FailingReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("pipe disconnected")))
    }
}

#[tokio::test]
async fn followup_without_a_saved_provider_session_is_never_restarted() {
    let (_f, mut adapter) = agent(LocalProvider::Codex).await;
    assert!(matches!(
        adapter.execute(&crate::test_support::send(12)).await,
        Err(NodeError::Rejected(
            "provider session is unavailable; automatic replay refused"
        ))
    ));
    assert!(
        adapter
            .journal
            .provider_binding(id(4))
            .await
            .expect("binding")
            .is_none()
    );
}
