use crate::tests::{
    common::{Fixture, Server},
    routing::{Selection, id, ready, request},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use orchestrator_coordinator::{routing::RoutingService, web};
use orchestrator_protocol::{
    attachments::{Attachment, AttachmentChunk, CHUNK_BYTES},
    ids::AttachmentId,
    routing::RouteAction,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
fn chunks(data: &[u8]) -> Vec<AttachmentChunk> {
    let attachment = Attachment {
        id: AttachmentId::random(),
        workspace_id: id(5),
        name: "data.txt".into(),
        media_type: "text/plain".into(),
        size: data.len() as u64,
        sha256: format!("{:x}", Sha256::digest(data)),
    };
    if data.is_empty() {
        return vec![AttachmentChunk {
            attachment,
            index: 0,
            data: String::new(),
        }];
    }
    data.chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(i, b)| AttachmentChunk {
            attachment: attachment.clone(),
            index: u32::try_from(i).expect("index"),
            data: STANDARD.encode(b),
        })
        .collect()
}
#[tokio::test]
async fn resumable_upload_is_verified_scoped_and_survives_store_reopen() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let chunks = chunks(&vec![42; CHUNK_BYTES + 3]);
    let file = &chunks[0].attachment;
    assert!(
        !f.store
            .put_attachment(&token, &chunks[1])
            .await
            .expect("out of order")
    );
    assert!(f.store.attachment(&token, file.id).await.is_err());
    assert!(
        f.store
            .put_attachment(&token, &chunks[0])
            .await
            .expect("complete")
    );
    assert!(
        f.store
            .put_attachment(&token, &chunks[1])
            .await
            .expect("exact retry")
    );
    let reopened = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("restart");
    assert_eq!(
        reopened
            .attachment(&token, file.id)
            .await
            .expect("metadata"),
        *file
    );
    assert_eq!(
        reopened
            .attachment_chunk(&token, file.id, 1)
            .await
            .expect("chunk")
            .data,
        chunks[1].data
    );
    reopened.close().await;
    let outsider = "f".repeat(64).try_into().expect("token");
    f.store
        .enroll(id(90), id(91), &outsider)
        .await
        .expect("tenant");
    assert!(f.store.attachment(&outsider, file.id).await.is_err());
    assert!(f.store.attachment_chunk(&token, file.id, 2).await.is_err());
    let mut changed = chunks[0].clone();
    changed.data = STANDARD.encode(vec![43; CHUNK_BYTES]);
    assert!(f.store.put_attachment(&token, &changed).await.is_err());
    changed = chunks[0].clone();
    changed.attachment.name = "changed.txt".into();
    assert!(f.store.put_attachment(&token, &changed).await.is_err());
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/v1/attachments", server.origin))
            .json(&chunks[0])
            .send()
            .await
            .expect("HTTP")
            .status(),
        401
    );
    assert!(
        client
            .post(format!("{}/v1/attachments", server.origin))
            .bearer_auth(token.expose())
            .json(&chunks[0])
            .send()
            .await
            .expect("HTTP")
            .status()
            .is_success()
    );
    let url = format!("{}/v1/attachments/{}/chunks/0", server.origin, file.id);
    assert_eq!(client.get(&url).send().await.expect("HTTP").status(), 401);
    assert!(
        client
            .get(&url)
            .bearer_auth(token.expose())
            .send()
            .await
            .expect("HTTP")
            .status()
            .is_success()
    );
    check_message_validation(&f, &token, file).await;
    f.close().await;
}
async fn check_message_validation(
    f: &Fixture,
    token: &orchestrator_protocol::sync::NodeToken,
    file: &Attachment,
) {
    let routing = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let mut request = request(300);
    if let RouteAction::Start { message, .. } = &mut request.action {
        message.attachments = vec![file.clone()];
    }
    routing
        .submit(token, request.clone())
        .await
        .expect("verified dispatch");
    let message = if let RouteAction::Start { message, .. } = &request.action {
        message.clone()
    } else {
        unreachable!()
    };
    request.action = RouteAction::Send {
        message: message.clone(),
    };
    f.store
        .validate_request_files(token, &request)
        .await
        .expect("follow-up");
    request.action = RouteAction::Restart {
        session_id: id(900),
        message,
    };
    f.store
        .validate_request_files(token, &request)
        .await
        .expect("restart");
    assert!(
        f.store
            .validate_attachments(token, id(55), std::slice::from_ref(file))
            .await
            .is_err()
    );
    assert!(
        f.store
            .validate_attachments(token, id(5), &vec![file.clone(); 9])
            .await
            .is_err()
    );
    let mut changed = file.clone();
    changed.name = "wrong.txt".into();
    assert!(
        f.store
            .validate_attachments(token, id(5), &[changed])
            .await
            .is_err()
    );
    request.thread_id = id(999);
    assert!(
        f.store
            .validate_request_files(token, &request)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn malformed_chunks_and_partial_files_never_become_ready() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let original = chunks(b"hello")[0].clone();
    for kind in 0..6 {
        let mut chunk = original.clone();
        match kind {
            0 => chunk.attachment.name = "../escape".into(),
            1 => chunk.index = 1,
            2 => chunk.data = "a".repeat(CHUNK_BYTES * 2),
            3 => chunk.data = "!".into(),
            4 => chunk.data = STANDARD.encode(b"shorter"),
            _ => chunk.attachment.sha256 = "f".repeat(64),
        }
        assert!(f.store.put_attachment(&token, &chunk).await.is_err());
    }
    assert!(
        f.store
            .attachment(&token, original.attachment.id)
            .await
            .is_err()
    );
    let empty = chunks(b"");
    assert!(
        f.store
            .put_attachment(&token, &empty[0])
            .await
            .expect("empty file")
    );
    f.close().await;
}

#[tokio::test]
async fn attachment_transactions_roll_back_each_failed_storage_stage() {
    for fault in [
        "ALTER TABLE attachments RENAME COLUMN metadata TO broken",
        "CREATE FUNCTION fail_attachment() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''failure''; END'; CREATE TRIGGER reject_chunk BEFORE INSERT ON attachment_chunks FOR EACH ROW EXECUTE FUNCTION fail_attachment()",
        "CREATE FUNCTION fail_attachment() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''failure''; END'; CREATE TRIGGER reject_ready BEFORE UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION fail_attachment()",
        "CREATE FUNCTION fail_attachment() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''failure''; END'; CREATE CONSTRAINT TRIGGER reject_commit AFTER INSERT ON attachment_chunks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_attachment()",
    ] {
        let f = Fixture::new().await;
        let token = ready(&f, 2, 4).await;
        f.sql(fault).await;
        assert!(
            f.store
                .put_attachment(&token, &chunks(b"content")[0])
                .await
                .is_err()
        );
        f.close().await;
    }
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let chunk = chunks(b"content")[0].clone();
    f.store.put_attachment(&token, &chunk).await.expect("put");
    f.sql("ALTER TABLE attachment_chunks RENAME COLUMN content TO broken")
        .await;
    assert!(
        f.store
            .attachment_chunk(&token, chunk.attachment.id, 0)
            .await
            .is_err()
    );
    f.sql("ALTER TABLE attachments RENAME COLUMN metadata TO broken")
        .await;
    assert!(
        f.store
            .attachment(&token, chunk.attachment.id)
            .await
            .is_err()
    );
    f.store.close().await;
    assert!(f.store.put_attachment(&token, &chunk).await.is_err());
    assert!(
        f.store
            .attachment(&token, chunk.attachment.id)
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn invalid_authentication_corrupt_storage_and_http_rejections_remain_errors() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let unknown = "d".repeat(64).try_into().expect("token");
    assert!(
        f.store
            .attachment_chunk(&token, AttachmentId::random(), 0)
            .await
            .is_err()
    );
    let chunk = chunks(b"data")[0].clone();
    assert!(f.store.put_attachment(&unknown, &chunk).await.is_err());
    let mut request = request(500);
    if let RouteAction::Start { message, .. } = &mut request.action {
        message.attachments = vec![chunk.attachment.clone()];
    }
    let routing = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    assert!(routing.submit(&token, request.clone()).await.is_err());
    if let RouteAction::Start { message, .. } = request.action {
        request.action = RouteAction::Send { message };
    }
    assert!(
        f.store
            .validate_request_files(&unknown, &request)
            .await
            .is_err()
    );
    f.store.put_attachment(&token, &chunk).await.expect("put");
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    let mut bad = chunk.clone();
    bad.data = "bad".into();
    assert!(
        !client
            .post(format!("{}/v1/attachments", server.origin))
            .bearer_auth(token.expose())
            .json(&bad)
            .send()
            .await
            .expect("response")
            .status()
            .is_success()
    );
    assert!(
        !client
            .get(format!(
                "{}/v1/attachments/{}/chunks/1",
                server.origin, chunk.attachment.id
            ))
            .bearer_auth(token.expose())
            .send()
            .await
            .expect("response")
            .status()
            .is_success()
    );
    f.sql("UPDATE attachments SET metadata='{}'").await;
    assert!(f.store.put_attachment(&token, &chunk).await.is_err());
    f.close().await;
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let parts = chunks(&vec![0; CHUNK_BYTES + 1]);
    f.store
        .put_attachment(&token, &parts[0])
        .await
        .expect("first");
    f.sql("ALTER TABLE attachment_chunks ALTER COLUMN content DROP NOT NULL; UPDATE attachment_chunks SET content=NULL").await;
    assert!(f.store.put_attachment(&token, &parts[1]).await.is_err());
    f.close().await;
}
