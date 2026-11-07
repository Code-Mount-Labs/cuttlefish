//! Tenant-isolated, checksum-verified file transport in the user's execution database.
use crate::{
    Error,
    store::{Store, authenticate},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use orchestrator_protocol::{
    attachments::{Attachment, AttachmentChunk, CHUNK_BYTES, MAX_FILES},
    ids::{AttachmentId, WorkspaceId},
    sync::NodeToken,
};
use sqlx::types::Json;
use uuid::Uuid;
fn invalid() -> Error {
    Error::RoutingConflict("attachment is invalid, unavailable or belongs to another workspace")
}
impl Store {
    /// Persist one file chunk atomically. Identical retries return the same readiness.
    ///
    /// # Errors
    /// Rejects invalid metadata, changed bytes, digest mismatches and storage failure.
    pub async fn put_attachment(
        &self,
        token: &NodeToken,
        chunk: &AttachmentChunk,
    ) -> Result<bool, Error> {
        chunk.attachment.validate().map_err(|_| invalid())?;
        let part = u64::from(chunk.index);
        if part >= chunk.attachment.chunks() || chunk.data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(invalid());
        }
        let bytes = STANDARD.decode(&chunk.data).map_err(|_| invalid())?;
        let remaining = chunk
            .attachment
            .size
            .saturating_sub(part * CHUNK_BYTES as u64);
        if bytes.len() as u64 != remaining.min(CHUNK_BYTES as u64) {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await?;
        let (group, _) = authenticate(&mut tx, token).await?;
        let id = Uuid::from(chunk.attachment.id);
        sqlx::query("INSERT INTO attachments(group_id,attachment_id,metadata) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(group).bind(id).bind(Json(&chunk.attachment)).execute(&mut *tx).await?;
        let (saved,ready):(Json<Attachment>,bool)=sqlx::query_as("SELECT metadata,ready FROM attachments WHERE group_id=$1 AND attachment_id=$2 FOR UPDATE").bind(group).bind(id).fetch_one(&mut *tx).await?;
        if saved.0 != chunk.attachment {
            return Err(invalid());
        }
        let accepted:Option<i32>=sqlx::query_scalar("INSERT INTO attachment_chunks(group_id,attachment_id,part,content) VALUES($1,$2,$3,$4) ON CONFLICT(group_id,attachment_id,part) DO UPDATE SET content=attachment_chunks.content WHERE attachment_chunks.content=excluded.content RETURNING part").bind(group).bind(id).bind(i64::from(chunk.index)).bind(&bytes).fetch_optional(&mut *tx).await?;
        if accepted.is_none() {
            return Err(invalid());
        }
        let chunks:Vec<Vec<u8>>=sqlx::query_scalar("SELECT content FROM attachment_chunks WHERE group_id=$1 AND attachment_id=$2 ORDER BY part").bind(group).bind(id).fetch_all(&mut *tx).await?;
        let complete = chunks.len() as u64 == chunk.attachment.chunks();
        if complete && !ready {
            let content: Vec<u8> = chunks.into_iter().flatten().collect();
            chunk.attachment.verify(&content).map_err(|_| invalid())?;
            sqlx::query("UPDATE attachments SET ready=TRUE WHERE group_id=$1 AND attachment_id=$2")
                .bind(group)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(complete)
    }
    /// Read metadata only after every chunk and its checksum have been verified.
    ///
    /// # Errors
    /// Rejects incomplete, unknown or cross-tenant files and unavailable storage.
    pub async fn attachment(
        &self,
        token: &NodeToken,
        id: AttachmentId,
    ) -> Result<Attachment, Error> {
        Ok(self.authenticated_attachment(token, id).await?.1)
    }
    async fn authenticated_attachment(
        &self,
        token: &NodeToken,
        id: AttachmentId,
    ) -> Result<(orchestrator_protocol::ids::GroupId, Attachment), Error> {
        let (group, _) = self.identity(token).await?;
        let value: Option<Json<Attachment>> = sqlx::query_scalar(
            "SELECT metadata FROM attachments WHERE group_id=$1 AND attachment_id=$2 AND ready",
        )
        .bind(Uuid::from(group))
        .bind(Uuid::from(id))
        .fetch_optional(&self.pool)
        .await?;
        Ok((group, value.ok_or_else(invalid)?.0))
    }
    /// Download one verified chunk through the same tenant authorization boundary.
    ///
    /// # Errors
    /// Rejects missing files, invalid chunk positions and storage failure.
    pub async fn attachment_chunk(
        &self,
        token: &NodeToken,
        id: AttachmentId,
        index: u32,
    ) -> Result<AttachmentChunk, Error> {
        let (group, attachment) = self.authenticated_attachment(token, id).await?;
        if u64::from(index) >= attachment.chunks() {
            return Err(invalid());
        }
        let data:Vec<u8>=sqlx::query_scalar("SELECT content FROM attachment_chunks WHERE group_id=$1 AND attachment_id=$2 AND part=$3").bind(Uuid::from(group)).bind(Uuid::from(id)).bind(i64::from(index)).fetch_one(&self.pool).await?;
        Ok(AttachmentChunk {
            attachment,
            index,
            data: STANDARD.encode(data),
        })
    }
    /// Validate every input before a chat message becomes dispatchable.
    ///
    /// # Errors
    /// Rejects too many files, uncommitted uploads, changed metadata and wrong workspaces.
    pub async fn validate_attachments(
        &self,
        token: &NodeToken,
        workspace: WorkspaceId,
        files: &[Attachment],
    ) -> Result<(), Error> {
        if files.len() > MAX_FILES {
            return Err(invalid());
        }
        for file in files {
            if file.workspace_id != workspace || self.attachment(token, file.id).await? != *file {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

impl Store {
    pub(crate) async fn validate_request_files(
        &self,
        token: &NodeToken,
        request: &orchestrator_protocol::routing::RouteRequest,
    ) -> Result<(), Error> {
        use orchestrator_protocol::routing::RouteAction;
        let message = match &request.action {
            RouteAction::Start { message, .. }
            | RouteAction::Send { message }
            | RouteAction::Restart { message, .. } => message,
            RouteAction::Cancel {} => return Ok(()),
        };
        if message.attachments.is_empty() {
            return Ok(());
        }
        let workspace = if let RouteAction::Start { workspace_id, .. } = request.action {
            workspace_id
        } else {
            let (group, _) = self.identity(token).await?;
            let value:Json<WorkspaceId>=sqlx::query_scalar("SELECT binding->'workspace_id' FROM thread_bindings WHERE group_id=$1 AND thread_id=$2").bind(Uuid::from(group)).bind(Uuid::from(request.thread_id)).fetch_one(&self.pool).await?;
            value.0
        };
        self.validate_attachments(token, workspace, &message.attachments)
            .await
    }
}
