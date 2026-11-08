//! Durable provider-to-router tool bridge. Provider processes never receive node credentials.
use crate::{NodeError, journal::Journal, sync::Remote};
use orchestrator_protocol::{
    ids::CommandId,
    tools::{ToolAction, ToolRequest},
};
use serde_json::{Value, json};
use sqlx::types::Json;
use std::time::Duration;
impl Journal {
    /// Save the exact tool request before transport, fenced by the executing worker.
    ///
    /// # Errors
    /// Rejects stale workers, identity reuse, oversized requests and storage failure.
    pub async fn stage_tool(
        &self,
        parent: CommandId,
        instance: &str,
        call: &str,
        action: ToolAction,
    ) -> Result<ToolRequest, NodeError> {
        if call.is_empty() || call.len() > 256 {
            return Err(NodeError::Rejected("invalid tool call identity"));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM command_receipts c JOIN local_sessions s USING(session_id) WHERE c.command_id=? AND c.status='executing' AND s.instance_id=? AND s.state='running')")
            .bind(parent.to_string()).bind(instance).fetch_one(&mut *tx).await?;
        if !owned {
            return Err(NodeError::Rejected("tool requires the executing worker"));
        }
        let request = ToolRequest {
            parent,
            id: CommandId::random(),
            action,
        };
        let Json(saved):Json<ToolRequest>=sqlx::query_scalar("INSERT INTO provider_tools(parent,call_id,request) VALUES(?,?,?) ON CONFLICT(parent,call_id) DO UPDATE SET request=provider_tools.request RETURNING request")
            .bind(parent.to_string()).bind(call).bind(Json(&request)).fetch_one(&mut *tx).await?;
        if saved.action != request.action {
            return Err(NodeError::Rejected(
                "tool identity conflicts with its original arguments",
            ));
        }
        tx.commit().await?;
        Ok(saved)
    }
    /// Read a persisted tool response after a supervisor restart.
    ///
    /// # Errors
    /// Reports unavailable storage or a missing invocation.
    pub async fn tool_response(
        &self,
        parent: CommandId,
        call: &str,
    ) -> Result<Option<Value>, NodeError> {
        let row: Option<Json<Value>> =
            sqlx::query_scalar("SELECT response FROM provider_tools WHERE parent=? AND call_id=?")
                .bind(parent.to_string())
                .bind(call)
                .fetch_one(&self.pool)
                .await?;
        Ok(row.map(|v| v.0))
    }
}
impl Remote {
    /// Relay saved requests with node authentication; failed transport retains the request.
    ///
    /// # Errors
    /// Reports unavailable local or remote storage without replaying a new identity.
    pub async fn flush_tools(&self, journal: &Journal) -> Result<(), NodeError> {
        let rows:Vec<(i64,Json<ToolRequest>)>=sqlx::query_as("SELECT sequence,request FROM provider_tools WHERE response IS NULL ORDER BY sequence LIMIT 4")
            .fetch_all(&journal.pool).await?;
        for (sequence, Json(request)) in rows {
            let response = match self
                .exchange(
                    "/v1/agent-tools",
                    reqwest::Method::POST,
                    Some(json!(request)),
                )
                .await
            {
                Ok(value) => json!({"value":value}),
                Err(NodeError::Transport(error))
                    if error.status().is_some_and(|s| s.is_client_error()) =>
                {
                    json!({"error":"The router rejected this collaboration request. Check that the parent is active, the workspace is in this group, and its permissions match."})
                }
                Err(error) => return Err(error),
            };
            sqlx::query(
                "UPDATE provider_tools SET response=? WHERE sequence=? AND response IS NULL",
            )
            .bind(Json(response))
            .bind(sequence)
            .execute(&journal.pool)
            .await?;
        }
        Ok(())
    }
}
/// Wait for a durable tool reply while the independent supervisor handles networking.
///
/// # Errors
/// Rejects malformed actions, stale workers, unavailable storage and timeout.
pub async fn invoke(
    journal: &Journal,
    parent: CommandId,
    instance: &str,
    call: &str,
    name: &str,
    args: Value,
) -> Result<Value, NodeError> {
    invoke_with_limit(journal, parent, instance, call, name, args, 600).await
}
async fn invoke_with_limit(
    journal: &Journal,
    parent: CommandId,
    instance: &str,
    call: &str,
    name: &str,
    args: Value,
    attempts: usize,
) -> Result<Value, NodeError> {
    let kind = match name {
        "cuttlefish_agents" => "agents",
        "cuttlefish_delegate" => "delegate",
        "cuttlefish_task" => "task",
        _ => return Err(NodeError::Rejected("unknown Cuttlefish tool")),
    };
    let mut args = args
        .as_object()
        .cloned()
        .ok_or(NodeError::Rejected("tool arguments must be an object"))?;
    if args.contains_key("type") {
        return Err(NodeError::Rejected(
            "tool arguments cannot override the operation",
        ));
    }
    args.insert("type".into(), json!(kind));
    let action = serde_json::from_value(json!(args))?;
    let staged = journal.stage_tool(parent, instance, call, action).await?;
    match wait_reply(journal, parent, call, attempts).await {
        Ok(value) => Ok(value),
        Err(NodeError::Rejected(_)) => Ok(
            json!({"pending":true,"request_id":staged.id,"task_id":if matches!(staged.action,ToolAction::Delegate{..}) {Some(staged.id)} else {None},"status":"Saved; waiting for the router. Do not create a duplicate delegation."}),
        ),
        Err(error) => Err(error),
    }
}
async fn wait_reply(
    journal: &Journal,
    parent: CommandId,
    call: &str,
    attempts: usize,
) -> Result<Value, NodeError> {
    for _ in 0..attempts {
        if let Some(reply) = journal.tool_response(parent, call).await? {
            return Ok(reply);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(NodeError::Rejected(
        "Collaboration is still waiting for the router. The request is saved; retry this same tool call instead of creating a duplicate task.",
    ))
}

#[cfg(test)]
#[path = "../tests/internal/tools.rs"]
mod tests;
