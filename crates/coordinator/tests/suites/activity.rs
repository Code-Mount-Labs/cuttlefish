use crate::tests::{
    common::{Fixture, Server},
    routing::{Selection, id, ready, request},
};
use orchestrator_coordinator::{routing::RoutingService, web};
use orchestrator_protocol::{
    activity::{ActivityPhase, ExecutionActivity},
    execution::LocalProvider,
    routing::DispatchPoll,
};
use std::sync::Arc;
fn event() -> ExecutionActivity {
    ExecutionActivity {
        sequence: 1,
        command_id: id(200),
        provider: LocalProvider::Claude,
        phase: ActivityPhase::ReadingFiles,
        at_ms: 1234,
    }
}
async fn dispatched(f: &Fixture) -> orchestrator_protocol::sync::NodeToken {
    let token = ready(f, 2, 4).await;
    RoutingService::new(f.store.clone(), Arc::new(Selection::default()))
        .submit(&token, request(200))
        .await
        .expect("route");
    assert!(
        f.store.record_activity(&token, &[event()]).await.is_err(),
        "queued work cannot claim execution"
    );
    f.store
        .poll_dispatch(
            &token,
            &DispatchPoll {
                workspaces: vec![id(5)],
                active_sessions: 0,
                accepted: vec![],
            },
        )
        .await
        .expect("claim");
    token
}
#[tokio::test]
async fn activity_is_authenticated_immutable_scoped_and_survives_reopen() {
    let f = Fixture::new().await;
    let token = dispatched(&f).await;
    let events = vec![event()];
    assert_eq!(
        f.store
            .record_activity(&token, &events)
            .await
            .expect("record"),
        vec![1]
    );
    assert_eq!(
        f.store
            .record_activity(&token, &events)
            .await
            .expect("retry"),
        vec![1]
    );
    let other = ready(&f, 9, 4).await;
    assert!(f.store.record_activity(&other, &events).await.is_err());
    let outsider = "f".repeat(64).try_into().expect("token");
    f.store
        .enroll(id(90), id(91), &outsider)
        .await
        .expect("tenant");
    assert!(f.store.record_activity(&outsider, &events).await.is_err());
    assert_eq!(
        f.store
            .conversation_activity(&outsider, id(201))
            .await
            .expect("scoped")["events"],
        serde_json::json!([])
    );
    let reopened = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("restart");
    let saved = reopened
        .conversation_activity(&token, id(201))
        .await
        .expect("read");
    assert_eq!(saved["events"], serde_json::json!(events));
    assert!(saved["last_seen_ms"].is_number());
    reopened.close().await;
    for sequence in [0, -1] {
        let mut bad = event();
        bad.sequence = sequence;
        assert!(f.store.record_activity(&token, &[bad]).await.is_err());
    }
    let mut bad = event();
    bad.at_ms = -1;
    assert!(f.store.record_activity(&token, &[bad]).await.is_err());
    assert!(
        f.store
            .record_activity(&token, &[event(), event()])
            .await
            .is_err()
    );
    assert!(
        f.store
            .record_activity(&token, &vec![event(); 65])
            .await
            .is_err()
    );
    let mut changed = event();
    changed.phase = ActivityPhase::Working;
    assert!(f.store.record_activity(&token, &[changed]).await.is_err());
    let unknown = "e".repeat(64).try_into().expect("token");
    assert!(f.store.record_activity(&unknown, &[]).await.is_err());
    assert!(
        f.store
            .conversation_activity(&unknown, id(201))
            .await
            .is_err()
    );
    check_http(&f, &token, &events).await;
    f.store.close().await;
    assert!(f.store.record_activity(&token, &[]).await.is_err());
    f.close().await;
}
#[tokio::test]
async fn activity_storage_failures_preserve_the_previous_timeline() {
    for fault in [
        "ALTER TABLE execution_activity RENAME COLUMN payload TO broken",
        "CREATE FUNCTION fail_activity() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''failure''; END'; CREATE CONSTRAINT TRIGGER reject_commit AFTER INSERT ON execution_activity DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_activity()",
    ] {
        let f = Fixture::new().await;
        let token = dispatched(&f).await;
        f.sql(fault).await;
        assert!(f.store.record_activity(&token, &[event()]).await.is_err());
        f.close().await;
    }
    let f = Fixture::new().await;
    let token = dispatched(&f).await;
    f.sql("ALTER TABLE execution_activity RENAME COLUMN payload TO broken")
        .await;
    assert!(
        f.store
            .conversation_activity(&token, id(201))
            .await
            .is_err()
    );
    f.sql("ALTER TABLE execution_activity RENAME COLUMN broken TO payload; ALTER TABLE nodes RENAME COLUMN last_seen TO broken").await;
    assert!(
        f.store
            .conversation_activity(&token, id(201))
            .await
            .is_err()
    );
    f.close().await;
}

async fn check_http(
    f: &Fixture,
    token: &orchestrator_protocol::sync::NodeToken,
    events: &[ExecutionActivity],
) {
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    let url = format!("{}/v1/activity", server.origin);
    assert_eq!(
        client
            .post(&url)
            .json(&events)
            .send()
            .await
            .expect("request")
            .status(),
        401
    );
    assert!(
        client
            .post(&url)
            .bearer_auth(token.expose())
            .json(&events)
            .send()
            .await
            .expect("request")
            .status()
            .is_success()
    );
    assert!(
        !client
            .post(&url)
            .bearer_auth(token.expose())
            .json(&vec![event(); 65])
            .send()
            .await
            .expect("request")
            .status()
            .is_success()
    );
}
