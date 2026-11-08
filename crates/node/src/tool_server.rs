//! A private, bounded MCP stdio server for one executing provider command.
use crate::{NodeError, journal::Journal};
use orchestrator_protocol::ids::CommandId;
use serde_json::{Value, json};
use std::io::Read;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
/// Describe the three scoped tools without exposing credentials or arbitrary endpoints.
#[must_use]
pub fn definitions() -> Value {
    json!([
        {"name":"cuttlefish_agents","description":include_str!("../../../prompts/collaboration/agents.md"),"inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"cuttlefish_delegate","description":include_str!("../../../prompts/collaboration/delegate.md"),"inputSchema":{"type":"object","properties":{"workspace_id":{"type":"string"},"message":{"type":"string","maxLength":4000},"acceptance":{"type":"string","maxLength":4000}},"required":["workspace_id","message","acceptance"],"additionalProperties":false}},
        {"name":"cuttlefish_task","description":include_str!("../../../prompts/collaboration/task.md"),"inputSchema":{"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"],"additionalProperties":false}}
    ])
}
/// Serve only requests attached to the supplied command and worker identity.
///
/// # Errors
/// Reports malformed/oversized transport frames, closed streams and local storage failure.
pub async fn serve(
    journal: &Journal,
    parent: CommandId,
    instance: &str,
    mut input: &mut (dyn AsyncBufRead + Unpin + Send),
    output: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<(), NodeError> {
    loop {
        let mut bytes = Vec::new();
        let count = (&mut input)
            .take(65_537)
            .read_until(b'\n', &mut bytes)
            .await?;
        if count == 0 {
            return Ok(());
        }
        if bytes.len() > 65_536 {
            return Err(NodeError::Rejected("tool frame exceeds 64 KiB"));
        }
        let request: Value = serde_json::from_slice(&bytes)?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = respond(journal, parent, instance, &request).await;
        let reply = match result {
            Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error.to_string()}})
            }
        };
        let mut bytes = reply.to_string().into_bytes();
        bytes.push(b'\n');
        output.write_all(&bytes).await?;
        output.flush().await?;
    }
}
async fn respond(
    journal: &Journal,
    parent: CommandId,
    instance: &str,
    request: &Value,
) -> Result<Value, NodeError> {
    match request["method"].as_str() {
        Some("initialize") => Ok(
            json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"cuttlefish","version":env!("CARGO_PKG_VERSION")}}),
        ),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({"tools":configured_definitions(journal, parent).await?})),
        Some("tools/call") => {
            let name = request["params"]["name"]
                .as_str()
                .ok_or(NodeError::Rejected("missing tool name"))?;
            let result = crate::tools::invoke(
                journal,
                parent,
                instance,
                &request["id"].to_string(),
                name,
                request["params"]
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .await;
            let (text, failed) = match result {
                Ok(value) => {
                    let failed = value.get("error").is_some();
                    (value.to_string(), failed)
                }
                Err(error) => (error.to_string(), true),
            };
            Ok(json!({"content":[{"type":"text","text":text}],"isError":failed}))
        }
        _ => Err(NodeError::Rejected("unsupported tool protocol method")),
    }
}

#[cfg(test)]
#[path = "../tests/internal/tool_server.rs"]
mod tests;

async fn configured_definitions(journal: &Journal, parent: CommandId) -> Result<Value, NodeError> {
    let mut tools = definitions();
    let command = journal
        .command(parent)
        .await?
        .ok_or(NodeError::Rejected("tool parent is missing"))?;
    if let Some(config) = journal
        .execution_config(command.request.target.session_id)
        .await?
    {
        let root = config
            .instructions
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or(NodeError::Rejected("invalid prompt directory"))?;
        for (index, name) in ["agents", "delegate", "task"].into_iter().enumerate() {
            let path = root.join("collaboration").join(format!("{name}.md"));
            if path.exists() {
                let mut text = String::new();
                std::fs::File::open(path)?
                    .take(8193)
                    .read_to_string(&mut text)?;
                if text.trim().is_empty() || text.len() > 8192 {
                    return Err(NodeError::Rejected(
                        "invalid collaboration tool description",
                    ));
                }
                tools[index]["description"] = json!(text);
            }
        }
    }
    Ok(tools)
}
