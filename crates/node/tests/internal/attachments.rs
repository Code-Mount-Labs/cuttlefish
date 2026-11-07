#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, start};
use axum::{
    Json, Router,
    extract::Path as RoutePath,
    routing::{get, post},
};
use orchestrator_protocol::execution::{LocalProvider, ProviderWorkspace};
fn file(data: &[u8]) -> Attachment {
    Attachment {
        id: AttachmentId::random(),
        workspace_id: id(5),
        name: "input.txt".into(),
        media_type: "text/plain".into(),
        size: data.len() as u64,
        sha256: format!("{:x}", Sha256::digest(data)),
    }
}
fn config(f: &Fixture) -> ExecutionConfig {
    let directory = f
        .journal
        .path()
        .parent()
        .expect("parent")
        .canonicalize()
        .expect("canonical");
    ExecutionConfig {
        workspace: ProviderWorkspace {
            workspace_id: id(5),
            name: "Files".into(),
            provider: LocalProvider::Codex,
            allow_edits: true,
        },
        executable: std::env::current_exe().expect("exe"),
        instructions: directory.join("instructions.md"),
        directory,
    }
}
async fn serve(app: Router) -> (crate::sync::Remote, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let remote =
        crate::sync::Remote::new(&url, &"a".repeat(64).try_into().expect("token")).expect("remote");
    (remote, server)
}
#[tokio::test]
async fn files_stage_before_execution_and_generated_artifacts_survive_reopen() {
    let f = Fixture::new().await;
    let config = config(&f);
    let input = file(b"hello");
    let mut command = start();
    if let AgentAction::Start { message, .. } = &mut command.action {
        message.attachments = vec![input.clone()];
    }
    let serve_input = input.clone();
    let uploaded = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AttachmentChunk>::new()));
    let saved = uploaded.clone();
    let app = Router::new()
        .route(
            "/v1/attachments/{id}/chunks/{index}",
            get(move |RoutePath((_, index)): RoutePath<(String, u32)>| {
                let attachment = serve_input.clone();
                async move {
                    Json(AttachmentChunk {
                        attachment,
                        index,
                        data: STANDARD.encode(b"hello"),
                    })
                }
            }),
        )
        .route(
            "/v1/attachments",
            post(move |Json(chunk): Json<AttachmentChunk>| {
                let saved = saved.clone();
                async move {
                    saved.lock().expect("lock").push(chunk);
                    Json(json!({"ready":true}))
                }
            }),
        );
    let (remote, server) = serve(app).await;
    remote
        .download_inputs(&f.journal, &command)
        .await
        .expect("download");
    remote
        .download_inputs(&f.journal, &command)
        .await
        .expect("retry");
    assert_eq!(
        std::fs::read(input_path(&config.directory, &input)).expect("file"),
        b"hello"
    );
    let context = input_context(&config, &command, "Examine this file").expect("context");
    assert!(context.contains("input_files"));
    assert!(!context.contains("hello"));
    let output = config
        .directory
        .join("artifacts")
        .join(command.command_id.to_string())
        .join("report.txt");
    std::fs::write(&output, b"result").expect("output");
    let results = collect_outputs(&f.journal, &config, &command)
        .await
        .expect("collect");
    assert_eq!(results.len(), 1);
    results[0].verify(b"result").expect("digest");
    let reopened = Journal::attach(f.journal.path()).await.expect("restart");
    remote.upload_artifacts(&reopened).await.expect("upload");
    remote
        .upload_artifacts(&reopened)
        .await
        .expect("no duplicate");
    assert_eq!(uploaded.lock().expect("lock").len(), 1);
    reopened.close().await;
    server.abort();
}
#[tokio::test]
async fn invalid_metadata_paths_and_checksums_never_reach_execution() {
    let f = Fixture::new().await;
    let config = config(&f);
    let input = file(b"hello");
    let mut command = start();
    if let AgentAction::Start { message, .. } = &mut command.action {
        message.attachments = vec![input.clone(); 9];
    }
    assert!(input_context(&config, &command, "read").is_err());
    let (remote, server) = serve(Router::new()).await;
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    if let AgentAction::Start { message, .. } = &mut command.action {
        let mut invalid = input.clone();
        invalid.workspace_id = id(50);
        message.attachments = vec![invalid];
    }
    assert!(input_context(&config, &command, "read").is_err());
    if let AgentAction::Start { message, .. } = &mut command.action {
        let mut invalid = input.clone();
        invalid.name = "../escape".into();
        message.attachments = vec![invalid];
    }
    assert!(input_context(&config, &command, "read").is_err());
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    assert_eq!(
        input_context(&config, &crate::test_support::cancel(9), "stop").expect("cancel"),
        "stop"
    );
    remote
        .download_inputs(&f.journal, &crate::test_support::cancel(9))
        .await
        .expect("no files");
    f.journal
        .save_artifact(&input, b"hello")
        .await
        .expect("save");
    f.journal
        .save_artifact(&input, b"hello")
        .await
        .expect("retry");
    assert!(f.journal.save_artifact(&input, b"wrong").await.is_err());
    let mut changed = input.clone();
    changed.name = "changed.txt".into();
    assert!(f.journal.save_artifact(&changed, b"hello").await.is_err());
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    server.abort();
}
#[tokio::test]
async fn output_collection_is_bounded_and_images_use_native_inputs() {
    let f = Fixture::new().await;
    let config = config(&f);
    let mut command = start();
    assert!(
        collect_outputs(&f.journal, &config, &command)
            .await
            .expect("no output")
            .is_empty()
    );
    input_context(&config, &command, "task").expect("context");
    let directory = config
        .directory
        .join("artifacts")
        .join(command.command_id.to_string());
    for name in [
        "image.png",
        "image.jpg",
        "image.webp",
        "report.pdf",
        "report.csv",
        "report.md",
        "report.docx",
        "report.xlsx",
    ] {
        std::fs::write(directory.join(name), b"file").expect("write");
    }
    assert_eq!(
        collect_outputs(&f.journal, &config, &command)
            .await
            .expect("all types")
            .len(),
        8
    );
    assert_eq!(media_type("data.bin"), "application/octet-stream");
    std::fs::write(directory.join("extra.txt"), b"extra").expect("extra");
    assert!(
        collect_outputs(&f.journal, &config, &command)
            .await
            .is_err()
    );
    let mut image = file(b"image");
    image.media_type = "image/png".into();
    if let AgentAction::Start { message, .. } = &mut command.action {
        message.attachments = vec![image.clone(), file(b"plain")];
    }
    let inputs = codex_input(&config, &command, "text");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[1]["type"], "localImage");
    assert_eq!(
        codex_input(&config, &crate::test_support::cancel(9), "text").len(),
        1
    );
    let oversized = config.directory.join("large");
    std::fs::File::create(&oversized)
        .expect("file")
        .set_len(orchestrator_protocol::attachments::MAX_FILE_BYTES + 1)
        .expect("size");
    assert!(read_bounded(&oversized).is_err());
}

#[tokio::test]
async fn download_rejects_substituted_truncated_corrupt_and_cross_workspace_files() {
    let f = Fixture::new().await;
    f.registered().await;
    let original = file(b"hello");
    let mut command = crate::test_support::send(10);
    if let AgentAction::Send { message } = &mut command.action {
        message.attachments = vec![original.clone()];
    }
    for kind in 0..7 {
        let mut chunk = AttachmentChunk {
            attachment: original.clone(),
            index: 0,
            data: STANDARD.encode(b"hello"),
        };
        match kind {
            0 => chunk.attachment.name = "other.txt".into(),
            1 => chunk.index = 1,
            2 => chunk.data = "a".repeat(CHUNK_BYTES * 2),
            3 => chunk.data = "!".into(),
            4 => chunk.data = STANDARD.encode(b"too long"),
            5 => chunk.data = STANDARD.encode(b"wrong"),
            _ => chunk.data.clear(),
        }
        let app = Router::new().route(
            "/v1/attachments/{id}/chunks/{index}",
            get(move || {
                let chunk = chunk.clone();
                async move { Json(chunk) }
            }),
        );
        let (remote, server) = serve(app).await;
        assert!(remote.download_inputs(&f.journal, &command).await.is_err());
        assert!(!input_path(&config(&f).directory, &original).exists());
        server.abort();
    }
    let (remote, server) = serve(Router::new()).await;
    if let AgentAction::Send { message } = &mut command.action {
        message.attachments[0].workspace_id = id(88);
    }
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    command.target.session_id = id(99);
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    server.abort();
}

#[tokio::test]
async fn pending_outbox_survives_remote_rejection_and_local_write_failure() {
    let f = Fixture::new().await;
    let input = file(b"hello");
    f.journal
        .save_artifact(&input, b"hello")
        .await
        .expect("save");
    let (remote, server) = serve(Router::new().route(
        "/v1/attachments",
        post(|| async { Json(json!({"ready":false})) }),
    ))
    .await;
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    server.abort();
    let (remote, server) = serve(Router::new().route(
        "/v1/attachments",
        post(|| async { Json(json!({"ready":true})) }),
    ))
    .await;
    let pool = f.raw().await;
    sqlx::query("CREATE TRIGGER fault BEFORE UPDATE ON local_artifacts BEGIN SELECT RAISE(ABORT,'write failed'); END").execute(&pool).await.expect("fault");
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    sqlx::query("DROP TRIGGER fault")
        .execute(&pool)
        .await
        .expect("restore");
    sqlx::query("UPDATE local_artifacts SET content=x'00'")
        .execute(&pool)
        .await
        .expect("corrupt");
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    pool.close().await;
    f.journal.close().await;
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    assert!(f.journal.save_artifact(&input, b"hello").await.is_err());
    server.abort();
}

#[tokio::test]
async fn attachment_paths_reject_symlink_replacement_and_preserve_existing_files() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new().await;
    let mut config = config(&f);
    let input = file(b"hello");
    let original = config.directory.clone();
    let alias = original.join("alias");
    symlink(&original, &alias).expect("alias");
    assert!(owned_directory(&alias).is_err());
    let output = original.join("artifacts");
    symlink(&original, &output).expect("output alias");
    assert!(input_context(&config, &start(), "test").is_err());
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    std::fs::remove_file(&output).expect("remove alias");
    let existing = original.join("existing");
    std::fs::write(&existing, b"wrong").expect("write");
    assert!(write_input(&existing, &input, b"hello").is_err());
    assert_eq!(std::fs::read(&existing).expect("read"), b"wrong");
    let alias_file = original.join("alias-file");
    symlink(&existing, &alias_file).expect("alias file");
    assert!(write_input(&alias_file, &input, b"hello").is_err());
    assert!(write_input(&original.join("missing/file"), &input, b"hello").is_err());
    assert!(read_bounded(&original.join("missing")).is_err());
    assert!(read_bounded(&original).is_err());
    config.workspace.allow_edits = false;
    input_context(&config, &start(), "read-only").expect("no output creation");
    assert!(!output.exists());
    let pool = f.raw().await;
    sqlx::query("UPDATE workspace_mounts SET path=?")
        .bind(alias.to_str())
        .execute(&pool)
        .await
        .expect("replace mount");
    assert!(
        f.journal
            .input_directory(&start(), std::slice::from_ref(&input))
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM workspace_mounts")
        .execute(&pool)
        .await
        .expect("remove mount");
    assert!(
        f.journal
            .input_directory(&start(), std::slice::from_ref(&input))
            .await
            .is_err()
    );
    pool.close().await;
}

#[tokio::test]
async fn unreadable_files_and_missing_storage_leave_inputs_unaccepted() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let config = config(&f);
    let input = file(b"hello");
    let command = {
        let mut c = start();
        if let AgentAction::Start { message, .. } = &mut c.action {
            message.attachments = vec![input.clone()];
        }
        c
    };
    let (remote, server) = serve(Router::new().route(
        "/v1/attachments/{id}/chunks/{index}",
        get(|| async { Json(json!({})) }),
    ))
    .await;
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    server.abort();
    let (remote, server) = serve(Router::new()).await;
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    server.abort();
    let metadata = input.clone();
    let (remote, server) = serve(Router::new().route(
        "/v1/attachments/{id}/chunks/{index}",
        get(move || {
            let attachment = metadata.clone();
            async move {
                Json(AttachmentChunk {
                    attachment,
                    index: 0,
                    data: STANDARD.encode(b"hello"),
                })
            }
        }),
    ))
    .await;
    let destination = input_path(&config.directory, &input);
    std::fs::create_dir_all(destination.parent().expect("parent")).expect("inputs");
    std::fs::write(&destination, b"different").expect("conflict");
    assert!(remote.download_inputs(&f.journal, &command).await.is_err());
    std::fs::remove_file(&destination).expect("remove");
    std::fs::create_dir(&destination).expect("directory");
    assert!(write_input(&destination, &input, b"hello").is_err());
    std::fs::remove_dir(&destination).expect("remove");
    input_context(&config, &start(), "test").expect("output");
    let output = config
        .directory
        .join("artifacts")
        .join(start().command_id.to_string());
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o000)).expect("mode");
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).expect("restore");
    let file_path = output.join("report.txt");
    std::fs::write(&file_path, b"result").expect("output");
    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o000)).expect("mode");
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600)).expect("restore");
    f.journal.close().await;
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    server.abort();
}

#[tokio::test]
async fn restart_retries_exact_artifact_chunks_after_a_partial_remote_commit() {
    let f = Fixture::new().await;
    let data = vec![42; CHUNK_BYTES + 3];
    let artifact = file(&data);
    f.journal
        .save_artifact(&artifact, &data)
        .await
        .expect("outbox");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AttachmentChunk>::new()));
    let requests = seen.clone();
    let (remote, server) = serve(Router::new().route(
        "/v1/attachments",
        post(move |Json(chunk): Json<AttachmentChunk>| {
            let requests = requests.clone();
            async move {
                let mut requests = requests.lock().expect("requests");
                requests.push(chunk.clone());
                let failed = requests.len() == 2;
                (
                    if failed {
                        axum::http::StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        axum::http::StatusCode::OK
                    },
                    Json(json!({"ready":chunk.index==1})),
                )
            }
        }),
    ))
    .await;
    assert!(remote.upload_artifacts(&f.journal).await.is_err());
    f.journal.close().await;
    let restarted = Journal::attach(f.journal.path()).await.expect("restart");
    remote
        .upload_artifacts(&restarted)
        .await
        .expect("resume upload");
    remote
        .upload_artifacts(&restarted)
        .await
        .expect("outbox acknowledged");
    {
        let requests = seen.lock().expect("requests");
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0], requests[2]);
        assert_eq!(requests[1], requests[3]);
    }
    restarted.close().await;
    server.abort();
}

#[tokio::test]
async fn output_enumeration_rejects_io_errors_and_non_utf8_names() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new().await;
    let config = config(&f);
    assert!(
        collect_entries(
            &f.journal,
            &config,
            &mut [Err(std::io::Error::other("directory disconnected"))].into_iter()
        )
        .await
        .is_err()
    );
    for path in [
        PathBuf::from("/"),
        config
            .directory
            .join(std::ffi::OsString::from_vec(vec![255])),
        config.directory.join("missing"),
    ] {
        assert!(
            collect_entries(&f.journal, &config, &mut [Ok(path)].into_iter())
                .await
                .is_err()
        );
    }
}
struct FailingFile {
    write_error: bool,
    sync_error: bool,
}
impl Write for FailingFile {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.write_error {
            Err(std::io::Error::other("disk full"))
        } else {
            Ok(data.len())
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl DurableFile for FailingFile {
    fn sync_data(&self) -> std::io::Result<()> {
        if self.sync_error {
            Err(std::io::Error::other("sync failed"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn atomic_input_publication_never_clobbers_and_cleans_failed_temporary_files() {
    for (write_error, sync_error) in [(true, false), (false, true), (false, false)] {
        assert_eq!(
            write_durable(
                &mut FailingFile {
                    write_error,
                    sync_error
                },
                b"input"
            )
            .is_err(),
            write_error || sync_error
        );
    }
    let dir = tempfile::tempdir().expect("directory");
    let destination = dir.path().join("input");
    std::fs::write(&destination, b"original").expect("original");
    let temporary = tempfile::NamedTempFile::new_in(dir.path()).expect("temporary");
    let temporary_path = temporary.path().to_owned();
    assert!(publish_input(temporary, &destination).is_err());
    assert!(!temporary_path.exists());
    assert_eq!(std::fs::read(destination).expect("original"), b"original");
}

#[test]
fn directory_sync_failure_keeps_a_verified_file_available_for_safe_retry() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("input");
    let input = file(b"hello");
    let mut temporary = tempfile::NamedTempFile::new_in(dir.path()).expect("temporary");
    write_durable(temporary.as_file_mut(), b"hello").expect("durable content");
    assert!(
        publish_synced(temporary, &path, |_| Err(std::io::Error::other(
            "sync failed"
        )))
        .is_err()
    );
    write_input(&path, &input, b"hello").expect("safe retry");
    assert!(sync_directory(&dir.path().join("missing")).is_err());
    assert!(write_input(Path::new(""), &input, b"hello").is_err());
    let temporary = tempfile::NamedTempFile::new_in(dir.path()).expect("temporary");
    assert!(publish_synced(temporary, Path::new(""), sync_directory).is_err());
}

#[tokio::test]
async fn unusable_workspace_paths_are_rejected_before_input_publication() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let config = config(&f);
    let input = file(b"hello");
    let pool = f.raw().await;
    sqlx::query("UPDATE workspace_mounts SET path=?")
        .bind(config.directory.join("missing").to_str())
        .execute(&pool)
        .await
        .expect("missing mount");
    assert!(
        f.journal
            .input_directory(&start(), std::slice::from_ref(&input))
            .await
            .is_err()
    );
    sqlx::query("UPDATE workspace_mounts SET path=?")
        .bind(config.directory.to_str())
        .execute(&pool)
        .await
        .expect("restore mount");
    std::fs::write(config.directory.join("attachments"), b"file").expect("obstruct directory");
    assert!(
        f.journal
            .input_directory(&start(), std::slice::from_ref(&input))
            .await
            .is_err()
    );
    let parent = config.directory.join("artifacts");
    std::fs::create_dir(&parent).expect("parent");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).expect("permissions");
    assert!(
        collect_outputs(&f.journal, &config, &start())
            .await
            .is_err()
    );
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).expect("restore");
    pool.close().await;
    f.journal.close().await;
    let profile = orchestrator_protocol::sync::NodeProfile {
        platform: orchestrator_protocol::capabilities::Platform::Linux,
        capabilities: std::collections::BTreeSet::default(),
        capacity: 1,
    };
    assert!(f.journal.sync_report(profile).await.is_err());
}

#[test]
fn failed_content_write_removes_temporary_files_without_publishing() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("input");
    let input = file(b"hello");
    assert!(
        write_input_using(&path, &input, b"hello", |_, _| Err(std::io::Error::other(
            "disk full"
        )
        .into()))
        .is_err()
    );
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).expect("entries").count(), 0);
    std::fs::write(&path, b"file").expect("obstruction");
    assert!(write_input(&path.join("child"), &input, b"hello").is_err());
}

#[tokio::test]
async fn followups_without_files_need_no_network_and_images_keep_native_input() {
    let f = Fixture::new().await;
    let config = config(&f);
    let remote = crate::sync::Remote::new(
        "http://127.0.0.1:1",
        &"a".repeat(64).try_into().expect("token"),
    )
    .expect("remote");
    let mut followup = crate::test_support::send(12);
    remote
        .download_inputs(&f.journal, &followup)
        .await
        .expect("no download required");
    let mut image = file(b"image");
    image.media_type = "image/png".into();
    if let AgentAction::Send { message } = &mut followup.action {
        message.attachments.push(image.clone());
    }
    let inputs = codex_input(&config, &followup, "edit this image");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], json!({"type":"text","text":"edit this image"}));
    assert_eq!(
        inputs[1],
        json!({"type":"localImage","path":input_path(&config.directory,&image)})
    );
}
