//! Durable artifact outbox and bounded input staging for local specialists.
use crate::{NodeError, execution::ExecutionConfig, journal::Journal};
use base64::{Engine, engine::general_purpose::STANDARD};
use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand},
    attachments::{Attachment, AttachmentChunk, CHUNK_BYTES, MAX_FILES},
    ids::AttachmentId,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
fn invalid() -> NodeError {
    NodeError::Rejected("attachment content or workspace is invalid")
}
impl Journal {
    /// Save generated content before reporting a completed task.
    ///
    /// # Errors
    /// Rejects changed identities, invalid content or unavailable storage.
    pub async fn save_artifact(&self, file: &Attachment, data: &[u8]) -> Result<(), NodeError> {
        file.verify(data).map_err(|_| invalid())?;
        let saved:Option<String>=sqlx::query_scalar("INSERT INTO local_artifacts(attachment_id,metadata,content) VALUES(?,?,?) ON CONFLICT(attachment_id) DO UPDATE SET attachment_id=local_artifacts.attachment_id WHERE local_artifacts.metadata=excluded.metadata AND local_artifacts.content=excluded.content RETURNING attachment_id").bind(file.id.to_string()).bind(Json(file)).bind(data).fetch_optional(&self.pool).await?;
        if saved.is_none() {
            return Err(invalid());
        }
        Ok(())
    }
    async fn input_directory(
        &self,
        command: &AgentCommand,
        files: &[Attachment],
    ) -> Result<PathBuf, NodeError> {
        let id = match &command.action {
            AgentAction::Start { workspace_id, .. } => workspace_id.to_string(),
            _ => {
                sqlx::query_scalar("SELECT workspace_id FROM local_sessions WHERE session_id=?")
                    .bind(command.target.session_id.to_string())
                    .fetch_one(&self.pool)
                    .await?
            }
        };
        for file in files {
            file.validate().map_err(|_| invalid())?;
            if file.workspace_id.to_string() != id {
                return Err(invalid());
            }
        }
        let path: String =
            sqlx::query_scalar("SELECT path FROM workspace_mounts WHERE workspace_id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let path = PathBuf::from(path);
        original_path(&path)?;
        let directory = path.join("attachments");
        owned_directory(&directory)?;
        Ok(directory)
    }
}
fn owned_directory(path: &Path) -> Result<(), NodeError> {
    std::fs::create_dir_all(path)?;
    original_path(path)
}
fn original_path(path: &Path) -> Result<(), NodeError> {
    if path.canonicalize()? != path {
        return Err(invalid());
    }
    Ok(())
}
/// Local attachment path uses a validated filename and an immutable identity.
#[must_use]
pub fn input_path(root: &Path, file: &Attachment) -> PathBuf {
    root.join("attachments")
        .join(format!("{}-{}", file.id, file.name))
}
/// Small structured context, never the full contents of an attachment.
///
/// # Errors
/// Reports invalid metadata or failure to prepare the task's output directory.
pub fn input_context(
    config: &ExecutionConfig,
    command: &AgentCommand,
    text: &str,
) -> Result<String, NodeError> {
    let files = match &command.action {
        AgentAction::Start { message, .. } | AgentAction::Send { message } => &message.attachments,
        AgentAction::Cancel => return Ok(text.into()),
    };
    if files.len() > MAX_FILES {
        return Err(invalid());
    }
    for file in files {
        file.validate().map_err(|_| invalid())?;
        if file.workspace_id != config.workspace.workspace_id {
            return Err(invalid());
        }
    }
    let output = config
        .directory
        .join("artifacts")
        .join(command.command_id.to_string());
    if config.workspace.allow_edits {
        owned_directory(&output)?;
    }
    let context = json!({"input_files":files.iter().map(|file|json!({"path":input_path(&config.directory,file),"name":file.name,"media_type":file.media_type})).collect::<Vec<_>>(),"output_directory":output});
    Ok(format!("{text}\n\n{context}"))
}
/// Snapshot task-specific generated files into the durable outbox.
///
/// # Errors
/// Rejects symlinks, too many files, oversized output or unavailable storage.
pub async fn collect_outputs(
    journal: &Journal,
    config: &ExecutionConfig,
    command: &AgentCommand,
) -> Result<Vec<Attachment>, NodeError> {
    let directory = config
        .directory
        .join("artifacts")
        .join(command.command_id.to_string());
    match directory.canonicalize() {
        Ok(actual) if actual == directory => {}
        Ok(_) => return Err(invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    }
    let mut entries = std::fs::read_dir(directory)?.map(|entry| entry.map(|entry| entry.path()));
    collect_entries(journal, config, &mut entries).await
}
async fn collect_entries(
    journal: &Journal,
    config: &ExecutionConfig,
    entries: &mut (dyn Iterator<Item = std::io::Result<PathBuf>> + Send),
) -> Result<Vec<Attachment>, NodeError> {
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(invalid)?
            .to_owned();
        let metadata = path.symlink_metadata()?;
        if !metadata.is_file()
            || files.len() >= MAX_FILES
            || metadata.len() > orchestrator_protocol::attachments::MAX_FILE_BYTES
        {
            return Err(invalid());
        }
        let data = read_bounded(&path)?;
        let file = Attachment {
            id: AttachmentId::random(),
            workspace_id: config.workspace.workspace_id,
            media_type: media_type(&name).into(),
            name,
            size: data.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&data)),
        };
        journal.save_artifact(&file, &data).await?;
        files.push(file);
    }
    Ok(files)
}
fn media_type(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "md" | "txt" => "text/plain",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}
impl crate::sync::Remote {
    /// Upload generated files before their completion events become visible remotely.
    ///
    /// # Errors
    /// Leaves the outbox pending on transport, checksum or journal failure.
    pub async fn upload_artifacts(&self, journal: &Journal) -> Result<(), NodeError> {
        for _ in 0..100 {
            let row:Option<(Json<Attachment>,Vec<u8>)>=sqlx::query_as("SELECT metadata,content FROM local_artifacts WHERE uploaded=0 ORDER BY attachment_id LIMIT 1").fetch_optional(&journal.pool).await?;
            let Some((Json(file), data)) = row else {
                break;
            };
            file.verify(&data).map_err(|_| invalid())?;
            for (index, start) in (0_u32..).zip((0..data.len().max(1)).step_by(CHUNK_BYTES)) {
                let end = (start + CHUNK_BYTES).min(data.len());
                let chunk = AttachmentChunk {
                    attachment: file.clone(),
                    index,
                    data: STANDARD.encode(&data[start..end]),
                };
                let response = self
                    .exchange("/v1/attachments", reqwest::Method::POST, Some(json!(chunk)))
                    .await?;
                if end == data.len() && response.get("ready") != Some(&json!(true)) {
                    return Err(invalid());
                }
            }
            sqlx::query("UPDATE local_artifacts SET uploaded=1 WHERE attachment_id=?")
                .bind(file.id.to_string())
                .execute(&journal.pool)
                .await?;
        }
        Ok(())
    }
    /// Materialize input files before accepting execution, with exact digest verification.
    ///
    /// # Errors
    /// Leaves dispatch unaccepted on download, metadata or local filesystem failure.
    pub async fn download_inputs(
        &self,
        journal: &Journal,
        command: &AgentCommand,
    ) -> Result<(), NodeError> {
        let files = match &command.action {
            AgentAction::Start { message, .. } | AgentAction::Send { message } => {
                &message.attachments
            }
            AgentAction::Cancel => return Ok(()),
        };
        if files.is_empty() {
            return Ok(());
        }
        if files.len() > MAX_FILES {
            return Err(invalid());
        }
        let directory = journal.input_directory(command, files).await?;
        for file in files {
            let mut data = Vec::new();
            for index in 0..file.chunks() {
                let value = self
                    .exchange(
                        &format!("/v1/attachments/{}/chunks/{index}", file.id),
                        reqwest::Method::GET,
                        None,
                    )
                    .await?;
                let chunk: AttachmentChunk = serde_json::from_value(value)?;
                if chunk.attachment != *file
                    || u64::from(chunk.index) != index
                    || chunk.data.len() > CHUNK_BYTES.div_ceil(3) * 4
                {
                    return Err(invalid());
                }
                data.extend(STANDARD.decode(chunk.data).map_err(|_| invalid())?);
                if data.len() as u64 > file.size {
                    return Err(invalid());
                }
            }
            file.verify(&data).map_err(|_| invalid())?;
            write_input(
                &directory.join(format!("{}-{}", file.id, file.name)),
                file,
                &data,
            )?;
        }
        Ok(())
    }
}
fn write_input(path: &Path, file: &Attachment, data: &[u8]) -> Result<(), NodeError> {
    write_input_using(path, file, data, write_durable)
}
fn write_input_using(
    path: &Path,
    file: &Attachment,
    data: &[u8],
    write: fn(&mut dyn DurableFile, &[u8]) -> Result<(), NodeError>,
) -> Result<(), NodeError> {
    let parent = path.parent().ok_or_else(invalid)?;
    match path.symlink_metadata() {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(invalid());
            }
            file.verify(&read_bounded(path)?).map_err(|_| invalid())?;
            return sync_directory(parent).map_err(NodeError::from);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write(temporary.as_file_mut(), data)?;
    publish_input(temporary, path)
}

trait DurableFile: Write {
    fn sync_data(&self) -> std::io::Result<()>;
}
impl DurableFile for std::fs::File {
    fn sync_data(&self) -> std::io::Result<()> {
        self.sync_all()
    }
}
fn write_durable(output: &mut dyn DurableFile, data: &[u8]) -> Result<(), NodeError> {
    output.write_all(data)?;
    output.sync_data()?;
    Ok(())
}
fn publish_input(temporary: tempfile::NamedTempFile, path: &Path) -> Result<(), NodeError> {
    publish_synced(temporary, path, sync_directory)
}
fn publish_synced(
    temporary: tempfile::NamedTempFile,
    path: &Path,
    sync: fn(&Path) -> std::io::Result<()>,
) -> Result<(), NodeError> {
    let parent = path.parent().ok_or_else(invalid)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| NodeError::from(error.error))?;
    sync(parent).map_err(NodeError::from)
}
#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path).and_then(|file| file.sync_all())
}
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Keeps the fallible directory-sync interface across platforms; std only provides directory fsync on Unix"
)]
fn sync_directory(_: &Path) -> std::io::Result<()> {
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, NodeError> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(orchestrator_protocol::attachments::MAX_FILE_BYTES + 1)
        .read_to_end(&mut data)?;
    if data.len() as u64 > orchestrator_protocol::attachments::MAX_FILE_BYTES {
        return Err(invalid());
    }
    Ok(data)
}
/// Native image inputs accompany small text metadata instead of base64 prompt text.
#[must_use]
pub fn codex_input(
    config: &ExecutionConfig,
    command: &AgentCommand,
    text: &str,
) -> Vec<serde_json::Value> {
    let mut inputs = vec![json!({"type":"text","text":text})];
    if let AgentAction::Start { message, .. } | AgentAction::Send { message } = &command.action {
        for file in &message.attachments {
            if matches!(
                file.media_type.as_str(),
                "image/png" | "image/jpeg" | "image/webp"
            ) {
                inputs.push(json!({"type":"localImage","path":input_path(&config.directory,file)}));
            }
        }
    }
    inputs
}

#[cfg(all(test, unix))]
#[path = "../tests/internal/attachments.rs"]
mod tests;
