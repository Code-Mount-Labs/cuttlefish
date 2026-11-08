#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, instance};
use orchestrator_protocol::tools::ToolAction;
use std::fmt::Write;
#[tokio::test]
async fn mcp_serves_bounded_scoped_requests_and_reports_errors() {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    let owner = instance().to_string();
    let input = [
        json!({"method":"notifications/initialized"}),
        json!({"id":1,"method":"initialize"}),
        json!({"id":2,"method":"ping"}),
        json!({"id":3,"method":"tools/list"}),
        json!({"id":4,"method":"invalid"}),
        json!({"id":5,"method":"tools/call","params":{}}),
        json!({"id":6,"method":"tools/call","params":{"name":"unknown"}}),
        json!({"id":7,"method":"tools/call","params":{"name":"cuttlefish_agents","arguments":{}}}),
    ];
    f.journal
        .stage_tool(id(6), &owner, &format!("{owner}:7"), ToolAction::Agents {})
        .await
        .expect("stage");
    sqlx::query("UPDATE provider_tools SET response=?")
        .bind(json!({"value":{"agents":[]}}).to_string())
        .execute(&f.journal.pool)
        .await
        .expect("response");
    let mut bytes = String::new();
    for value in input {
        writeln!(bytes, "{value}").expect("frame");
    }
    let mut output = Vec::new();
    serve(
        &f.journal,
        id(6),
        &owner,
        &mut std::io::Cursor::new(bytes),
        &mut output,
    )
    .await
    .expect("MCP");
    let replies: Vec<Value> = String::from_utf8(output)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(replies.len(), 7);
    assert_eq!(
        replies[2]["result"]["tools"]
            .as_array()
            .expect("tools")
            .len(),
        3
    );
    assert_eq!(replies[6]["result"]["isError"], false);
    assert_eq!(replies[5]["result"]["isError"], true);
    assert!(replies[3].get("error").is_some());
    for input in ["x".repeat(65_537), "invalid\n".to_owned()] {
        assert!(
            serve(
                &f.journal,
                id(6),
                &owner,
                &mut std::io::Cursor::new(input),
                &mut Vec::new()
            )
            .await
            .is_err()
        );
    }
}
#[tokio::test]
async fn tool_descriptions_are_editable_bounded_and_fail_closed() {
    use crate::execution::ExecutionConfig;
    use orchestrator_protocol::execution::{LocalProvider, ProviderWorkspace};
    let f = Fixture::new().await;
    let root = f.temp.path().canonicalize().expect("root");
    let execution = root.join("execution");
    let descriptions = root.join("collaboration");
    std::fs::create_dir_all(&execution).expect("execution");
    std::fs::create_dir_all(&descriptions).expect("descriptions");
    let instructions = execution.join("specialist.md");
    std::fs::write(&instructions, "Test specialist").expect("prompt");
    let config = ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Test".into(),
            provider: LocalProvider::Codex,
            allow_edits: false,
        },
        executable: std::env::current_exe().expect("exe"),
        directory: root,
        instructions,
    };
    f.journal
        .configure_execution(&config)
        .await
        .expect("configuration");
    f.registered().await;
    assert!(configured_definitions(&f.journal, id(999)).await.is_err());
    assert_eq!(
        configured_definitions(&f.journal, id(6))
            .await
            .expect("bundled"),
        definitions()
    );
    let file = descriptions.join("agents.md");
    std::fs::write(&file, "Organization-reviewed discovery instructions").expect("edit");
    assert_eq!(
        configured_definitions(&f.journal, id(6))
            .await
            .expect("edited")[0]["description"],
        "Organization-reviewed discovery instructions"
    );
    for bytes in [vec![], vec![255], vec![b'a'; 8193]] {
        std::fs::write(&file, bytes).expect("invalid prompt");
        assert!(configured_definitions(&f.journal, id(6)).await.is_err());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000))
            .expect("deny access");
        assert!(configured_definitions(&f.journal, id(6)).await.is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
            .expect("restore access");
    }
    std::fs::remove_file(&file).expect("remove");
    std::fs::create_dir(&file).expect("not a file");
    assert!(configured_definitions(&f.journal, id(6)).await.is_err());
    sqlx::query(
        "UPDATE workspace_providers SET configuration=json_set(configuration,'$.instructions','/')",
    )
    .execute(&f.journal.pool)
    .await
    .expect("invalid path");
    assert!(configured_definitions(&f.journal, id(6)).await.is_err());
    f.journal.close().await;
    assert!(configured_definitions(&f.journal, id(6)).await.is_err());
}

#[derive(Default)]
struct BrokenOutput {
    flush: bool,
}
impl tokio::io::AsyncWrite for BrokenOutput {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(if self.flush {
            Ok(bytes.len())
        } else {
            Err(std::io::Error::other("write failed"))
        })
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("flush failed")))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
#[tokio::test]
async fn tool_transport_and_configuration_failures_are_explicit() {
    let f = Fixture::new().await;
    for flush in [false, true] {
        assert!(
            serve(
                &f.journal,
                id(6),
                "owner",
                &mut std::io::Cursor::new(b"{\"id\":1,\"method\":\"ping\"}\n"),
                &mut BrokenOutput { flush }
            )
            .await
            .is_err()
        );
    }
    let mut input = tokio::io::BufReader::new(BrokenInput);
    assert!(
        serve(&f.journal, id(6), "owner", &mut input, &mut Vec::new())
            .await
            .is_err()
    );
    let mut out = Vec::new();
    serve(
        &f.journal,
        id(6),
        "owner",
        &mut std::io::Cursor::new(b"{\"id\":1,\"method\":\"tools/list\"}\n"),
        &mut out,
    )
    .await
    .expect("error frame");
    assert!(String::from_utf8(out).expect("utf8").contains("error"));
    f.registered().await;
    sqlx::query("ALTER TABLE workspace_providers RENAME COLUMN configuration TO hidden")
        .execute(&f.journal.pool)
        .await
        .expect("fault");
    assert!(configured_definitions(&f.journal, id(6)).await.is_err());
}

struct BrokenInput;
impl tokio::io::AsyncRead for BrokenInput {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::Error::other("read failed")))
    }
}
