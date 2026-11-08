//! Authenticated workspace discovery and conversation history over routing records.
use crate::{
    Error,
    store::{Store, authenticate},
};
use orchestrator_protocol::{
    execution::ProviderWorkspace, ids::ThreadId, routing::DispatchView, sync::NodeToken,
};
use serde_json::Value;
use sqlx::types::Json;
use uuid::Uuid;
impl Store {
    /// Advertise bounded, immutable workspace metadata for the authenticated node.
    ///
    /// # Errors
    /// Rejects ownership changes, invalid labels and storage failures.
    pub async fn advertise_workspaces(
        &self,
        token: &NodeToken,
        workspaces: &[ProviderWorkspace],
    ) -> Result<(), Error> {
        if workspaces.len() > 100 {
            return Err(Error::RoutingConflict("too many workspaces"));
        }
        let mut tx = self.pool.begin().await?;
        let (group, node) = authenticate(&mut tx, token).await?;
        for workspace in workspaces {
            if workspace.name.trim().is_empty() || workspace.name.len() > 128 {
                return Err(Error::RoutingConflict("invalid workspace name"));
            }
            let saved: Option<Uuid> = sqlx::query_scalar("INSERT INTO provider_workspaces(group_id,workspace_id,node_id,workspace) VALUES($1,$2,$3,$4) ON CONFLICT(group_id,workspace_id) DO UPDATE SET workspace=provider_workspaces.workspace WHERE provider_workspaces.node_id=excluded.node_id AND provider_workspaces.workspace=excluded.workspace RETURNING node_id")
                .bind(group).bind(Uuid::from(workspace.workspace_id)).bind(node).bind(Json(workspace)).fetch_optional(&mut *tx).await?;
            if saved.is_none() {
                return Err(Error::RoutingConflict(
                    "workspace ownership or configuration conflict",
                ));
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// Discover provider workspaces without disclosing machine paths or credentials.
    ///
    /// # Errors
    /// Reports authentication or storage failures.
    pub async fn provider_workspaces(&self, token: &NodeToken) -> Result<Vec<Value>, Error> {
        let (group, _) = self.identity(token).await?;
        let rows: Vec<Json<Value>> = sqlx::query_scalar("SELECT jsonb_build_object('node_id',node_id,'workspace',workspace) FROM provider_workspaces WHERE group_id=$1 ORDER BY workspace_id LIMIT 100")
            .bind(Uuid::from(group)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
    /// Page recent conversations from the durable dispatch log.
    ///
    /// # Errors
    /// Reports authentication or storage failures.
    pub async fn conversations(&self, token: &NodeToken) -> Result<Vec<Value>, Error> {
        let (group, _) = self.identity(token).await?;
        let rows: Vec<Json<Value>> = sqlx::query_scalar("SELECT jsonb_build_object('thread_id',t.thread_id,'workspace_id',t.binding->'workspace_id','state',d.state,'request',d.request,'outcome',d.outcome,'title',(SELECT request->'action'->'message'->>'text' FROM dispatches first_turn WHERE first_turn.group_id=t.group_id AND first_turn.thread_id=t.thread_id AND first_turn.request->'action'->'message' IS NOT NULL ORDER BY first_turn.sequence LIMIT 1)) FROM thread_bindings t JOIN LATERAL (SELECT state,request,outcome,sequence FROM dispatches WHERE group_id=t.group_id AND thread_id=t.thread_id ORDER BY sequence DESC LIMIT 1) d ON TRUE WHERE t.group_id=$1 ORDER BY d.sequence DESC LIMIT 100")
            .bind(Uuid::from(group)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
    /// Read up to 100 most recent turns in chronological order.
    ///
    /// # Errors
    /// Reports authentication, decoding or storage failures.
    pub async fn conversation(
        &self,
        token: &NodeToken,
        thread: ThreadId,
    ) -> Result<Vec<DispatchView>, Error> {
        let (group, _) = self.identity(token).await?;
        let rows: Vec<Json<DispatchView>> = sqlx::query_scalar("SELECT view FROM (SELECT sequence,jsonb_build_object('request',request,'command',command,'state',state,'outcome',outcome) AS view FROM dispatches WHERE group_id=$1 AND thread_id=$2 ORDER BY sequence DESC LIMIT 100) recent ORDER BY sequence")
            .bind(Uuid::from(group)).bind(Uuid::from(thread)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
}

impl Store {
    /// Read child task placement and execution for one authenticated conversation.
    ///
    /// # Errors
    /// Reports authentication and storage failures.
    pub async fn conversation_tasks(
        &self,
        token: &NodeToken,
        thread: ThreadId,
    ) -> Result<Vec<Value>, Error> {
        let (group, _) = self.identity(token).await?;
        let rows:Vec<Json<Value>>=sqlx::query_scalar("SELECT jsonb_build_object('task_id',t.task_id,'thread_id',t.thread_id,'node_id',d.node_id,'state',COALESCE(d.state,'planned'),'title',s#>>'{spec,acceptance}','workspace',w.workspace->>'name','provider',w.workspace->>'provider','platform',n.profile->>'platform','outcome',d.outcome) FROM goals g JOIN goal_tasks t ON t.group_id=g.group_id AND t.goal_id=g.goal_id CROSS JOIN LATERAL jsonb_array_elements(g.snapshot->'tasks') s LEFT JOIN dispatches d ON d.group_id=t.group_id AND d.command_id=t.command_id LEFT JOIN provider_workspaces w ON w.group_id=t.group_id AND w.workspace_id=(s#>>'{spec,request,action,workspace_id}')::uuid LEFT JOIN nodes n ON n.group_id=t.group_id AND n.node_id=d.node_id WHERE g.group_id=$1 AND g.thread_id=$2 AND s#>>'{spec,request,command_id}'=t.command_id::text ORDER BY t.task_id")
            .bind(Uuid::from(group)).bind(Uuid::from(thread)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
}
