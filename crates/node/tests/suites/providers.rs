//! Exercise provider failures through the public runtime, as a separate consumer crate.
#![allow(clippy::expect_used)]
use crate::common::{Fixture, id, instance, send, start};
use orchestrator_agents::Agent;
use orchestrator_node::{
    execution::ExecutionConfig, local_provider::LocalAgent, model::CommandStatus, worker,
};
use orchestrator_protocol::execution::{LocalProvider, ProviderWorkspace};
use std::os::unix::fs::PermissionsExt;
fn configuration(f: &Fixture, provider: LocalProvider, body: &str) -> ExecutionConfig {
    let directory = f.temp.path().canonicalize().expect("directory");
    let executable = directory.join("provider");
    std::fs::write(&executable, format!("#!/bin/sh\n{body}\n")).expect("provider");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("executable");
    let instructions = directory.join("prompt.md");
    std::fs::write(&instructions, "Test").expect("instructions");
    ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Provider test".into(),
            provider,
            allow_edits: false,
        },
        executable,
        directory,
        instructions,
    }
}
#[tokio::test]
async fn provider_process_failures_are_uncertain_and_survive_journal_reopen() {
    for provider in [LocalProvider::Codex, LocalProvider::Claude] {
        let f = Fixture::new().await;
        let config = configuration(&f, provider, "cat >/dev/null\nexit 1");
        // Codex waits for a protocol response, so fail immediately instead of draining stdin.
        if provider == LocalProvider::Codex {
            std::fs::write(&config.executable, "#!/bin/sh\nexit 1\n").expect("provider");
        }
        f.accepted().await;
        let adapter = LocalAgent::new(f.journal.clone(), instance().to_string(), config);
        assert!(
            worker::run(&f.journal, id(4), &instance().to_string(), adapter)
                .await
                .is_err()
        );
        let reopened = orchestrator_node::journal::Journal::attach(f.journal.path())
            .await
            .expect("restart");
        assert_eq!(
            reopened
                .command(id(6))
                .await
                .expect("command")
                .expect("receipt")
                .status,
            CommandStatus::Uncertain
        );
        assert!(reopened.pending_events().await.expect("events").is_empty());
        reopened.close().await;
    }
}
#[tokio::test]
async fn public_provider_adapter_rejects_missing_binding_failed_exit_and_invalid_instructions() {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    let response = serde_json::json!({"type":"result","session_id":id::<orchestrator_protocol::ids::SessionId>(4),"is_error":false,"subtype":"success","result":"Done"});
    let config = configuration(
        &f,
        LocalProvider::Claude,
        &format!("cat >/dev/null\nprintf '%s\\n' '{response}'\nexit 1"),
    );
    let mut agent = LocalAgent::new(f.journal.clone(), instance().to_string(), config);
    assert!(agent.execute(&send(10)).await.is_err());
    assert!(agent.execute(&start()).await.is_err());
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    let config = configuration(
        &f,
        LocalProvider::Codex,
        "printf '%s\\n' '{\"id\":1,\"result\":{}}'\ncat >/dev/null",
    );
    for bytes in [vec![255], vec![]] {
        std::fs::write(&config.instructions, bytes).expect("invalid instructions");
        let mut agent = LocalAgent::new(f.journal.clone(), instance().to_string(), config.clone());
        assert!(agent.execute(&start()).await.is_err());
    }
    f.journal.close().await;
    let profile = orchestrator_protocol::sync::NodeProfile {
        platform: orchestrator_protocol::capabilities::Platform::Linux,
        capabilities: std::collections::BTreeSet::new(),
        capacity: 1,
    };
    assert!(f.journal.sync_report(profile).await.is_err());
}

#[tokio::test]
async fn public_attachment_api_rejects_bad_inputs_and_corrupt_outbox_content() {
    use orchestrator_node::attachments::{collect_outputs, input_context};
    use orchestrator_protocol::{agent::AgentAction, attachments::Attachment, ids::AttachmentId};
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    let mut config = configuration(&f, LocalProvider::Claude, "exit 1");
    config.workspace.allow_edits = true;
    let file = Attachment {
        id: AttachmentId::random(),
        workspace_id: id(5),
        name: "input.txt".into(),
        media_type: "text/plain".into(),
        size: 5,
        sha256: format!("{:x}", Sha256::digest(b"hello")),
    };
    assert!(f.journal.save_artifact(&file, b"wrong").await.is_err());
    f.journal
        .save_artifact(&file, b"hello")
        .await
        .expect("outbox");
    let mut command = start();
    if let AgentAction::Start { message, .. } = &mut command.action {
        let mut invalid = file.clone();
        invalid.name = "../bad".into();
        message.attachments = vec![invalid];
    }
    assert!(input_context(&config, &command, "test").is_err());
    input_context(&config, &start(), "test").expect("output directory");
    let output = config
        .directory
        .join("artifacts")
        .join(start().command_id.to_string());
    std::fs::create_dir(output.join("nested")).expect("invalid nested artifact");
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    let pool = f.raw().await;
    sqlx::query("UPDATE local_artifacts SET content=x'00'")
        .execute(&pool)
        .await
        .expect("corrupt");
    let token = "a".repeat(64).try_into().expect("token");
    let remote =
        orchestrator_node::sync::Remote::new("http://127.0.0.1:1", &token).expect("remote");
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    pool.close().await;
}
