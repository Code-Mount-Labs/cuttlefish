#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{common::Fixture, data};
use serde_json::json;

fn request() -> RouteRequest {
    serde_json::from_value(json!({"command_id":Uuid::from_u128(100),"thread_id":Uuid::from_u128(101),"action":{"type":"start","task_id":Uuid::from_u128(102),"session_id":Uuid::from_u128(103),"workspace_id":Uuid::from_u128(5),"capabilities":["code.rust"],"message":{"id":Uuid::from_u128(104),"text":"work"}}})).expect("request")
}
fn poll() -> DispatchPoll {
    DispatchPoll {
        workspaces: vec![Uuid::from_u128(5).try_into().expect("id")],
        active_sessions: 0,
        accepted: vec![],
    }
}
async fn fixture() -> Fixture {
    let f = Fixture::enrolled().await;
    let mut report = data::report();
    report.events.clear();
    f.store
        .sync(&data::token(), &report)
        .await
        .expect("profile");
    f.store
        .poll_dispatch(&data::token(), &poll())
        .await
        .expect("ready");
    report.node_id = Uuid::from_u128(9).try_into().expect("id");
    let b: NodeToken = "b".repeat(64).try_into().expect("token");
    f.store
        .enroll(report.group_id, report.node_id, &b)
        .await
        .expect("peer");
    f.store.sync(&b, &report).await.expect("profile");
    f.store.poll_dispatch(&b, &poll()).await.expect("ready");
    f
}
async fn plan(f: &Fixture, input: Input) -> Plan {
    let Prepared::Choose(plan) = f
        .store
        .prepare(&data::token(), input)
        .await
        .expect("prepare")
    else {
        unreachable!()
    };
    *plan
}
async fn start(f: &Fixture) -> DispatchView {
    let plan = plan(f, Input::Submit(request())).await;
    f.store
        .commit_route(&data::token(), plan, data::report().node_id)
        .await
        .expect("commit")
}
fn send() -> RouteRequest {
    let mut request = request();
    request.command_id = Uuid::from_u128(105).try_into().expect("id");
    let RouteAction::Start { message, .. } = request.action else {
        unreachable!()
    };
    request.action = RouteAction::Send { message };
    request
}
async fn completed(f: &Fixture) {
    f.sql("UPDATE dispatches SET state='completed',outcome='{\"type\":\"rejected\",\"reason\":\"test\"}'").await;
}

#[tokio::test]
async fn commit_rechecks_authentication_revision_and_checked_counters() {
    let f = fixture().await;
    let p = plan(&f, Input::Submit(request())).await;
    f.store
        .revoke(data::report().group_id, data::report().node_id)
        .await
        .expect("revoke");
    assert!(matches!(
        f.store
            .commit_route(&data::token(), p, data::report().node_id)
            .await,
        Err(Error::Unauthorized)
    ));
    f.close().await;
    let f = fixture().await;
    start(&f).await;
    completed(&f).await;
    let p = plan(&f, Input::Submit(send())).await;
    f.sql("UPDATE thread_bindings SET revision=revision+1")
        .await;
    assert!(matches!(
        f.store
            .commit_route(&data::token(), p, data::report().node_id)
            .await,
        Err(Error::RoutingConflict(_))
    ));
    f.sql("UPDATE thread_bindings SET revision=9223372036854775807")
        .await;
    let p = plan(&f, Input::Submit(send())).await;
    assert!(matches!(
        f.store
            .commit_route(&data::token(), p, data::report().node_id)
            .await,
        Err(Error::RoutingConflict(_))
    ));
    f.close().await;
    let f = fixture().await;
    f.sql("INSERT INTO groups(group_id) VALUES ('00000000-0000-0000-0000-000000000000'); UPDATE nodes SET group_id='00000000-0000-0000-0000-000000000000'").await;
    let p = plan(&f, Input::Submit(request())).await;
    assert!(matches!(
        f.store
            .commit_route(&data::token(), p, data::report().node_id)
            .await,
        Err(Error::Unauthorized)
    ));
    f.close().await;
}

#[tokio::test]
async fn failover_rejects_epoch_overflow_and_rolls_back_failed_dispatch_update() {
    for overflow in [false, true] {
        let f = fixture().await;
        start(&f).await;
        f.sql("UPDATE nodes SET dispatch_seen=clock_timestamp()-interval '1 hour' WHERE node_id='00000000-0000-0000-0000-000000000002'").await;
        if overflow {
            f.sql("UPDATE thread_bindings SET binding=jsonb_set(binding,'{target,execution_epoch}','18446744073709551615')").await;
        } else {
            f.sql("ALTER TABLE dispatches ADD CONSTRAINT keep_owner CHECK(node_id='00000000-0000-0000-0000-000000000002')").await;
        }
        let p = plan(&f, Input::Failover(request().thread_id)).await;
        let result = f
            .store
            .commit_route(
                &data::token(),
                p,
                Uuid::from_u128(9).try_into().expect("id"),
            )
            .await;
        assert!(result.is_err());
        let saved = f
            .store
            .thread(&data::token(), request().thread_id)
            .await
            .expect("saved");
        assert_eq!(saved.command.node_id, data::report().node_id);
        f.close().await;
    }
    assert!(
        assignment(
            &Input::Failover(request().thread_id),
            &request().action,
            None
        )
        .is_err()
    );
}

#[tokio::test]
async fn stage_read_errors_are_propagated() {
    for fault in [
        "ALTER TABLE dispatches RENAME COLUMN state TO hidden",
        "ALTER TABLE dispatches RENAME COLUMN command TO hidden",
    ] {
        let f = fixture().await;
        f.sql(fault).await;
        let (mut tx, group, _) = f
            .store
            .routing_transaction(&data::token())
            .await
            .expect("tx");
        assert!(validate_request(&mut tx, group, &request()).await.is_err());
        drop(tx);
        assert!(
            f.store
                .thread(&data::token(), request().thread_id)
                .await
                .is_err()
        );
        f.close().await;
    }
    let f = fixture().await;
    start(&f).await;
    f.sql("ALTER TABLE nodes RENAME COLUMN last_seen TO hidden")
        .await;
    assert!(
        f.store
            .prepare(&data::token(), Input::Failover(request().thread_id))
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn polling_rolls_back_readiness_acceptance_and_claim_on_every_write_failure() {
    for fault in [
        "ALTER TABLE nodes ADD CONSTRAINT reject_ready CHECK(dispatch_seen IS NULL) NOT VALID",
        "ALTER TABLE dispatches ADD CONSTRAINT reject_claim CHECK(state<>'claimed') NOT VALID",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER UPDATE ON nodes DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
    ] {
        let f = fixture().await;
        start(&f).await;
        f.sql(fault).await;
        assert!(
            f.store
                .poll_dispatch(&data::token(), &poll())
                .await
                .is_err()
        );
        let view = f
            .store
            .thread(&data::token(), request().thread_id)
            .await
            .expect("saved");
        assert_eq!(view.state, DeliveryState::Queued);
        f.close().await;
    }
    let f = fixture().await;
    start(&f).await;
    f.store
        .poll_dispatch(&data::token(), &poll())
        .await
        .expect("claim");
    f.sql("ALTER TABLE dispatches ADD CONSTRAINT reject_acceptance CHECK(NOT accepted)")
        .await;
    assert!(
        f.store
            .poll_dispatch(
                &data::token(),
                &DispatchPoll {
                    accepted: vec![request().command_id],
                    ..poll()
                }
            )
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn outcome_write_failure_preserves_original_dispatch_and_thread() {
    for fault in [
        "ALTER TABLE dispatches RENAME COLUMN outcome TO hidden",
        "ALTER TABLE dispatches ADD CONSTRAINT reject_completed CHECK(state<>'completed')",
        "ALTER TABLE thread_bindings ADD CONSTRAINT reject_stop CHECK(NOT stopped)",
    ] {
        let f = fixture().await;
        start(&f).await;
        completed(&f).await;
        let mut cancel = send();
        cancel.action = RouteAction::Cancel {};
        let p = plan(&f, Input::Submit(cancel)).await;
        let view = f
            .store
            .commit_route(&data::token(), p, data::report().node_id)
            .await
            .expect("cancel");
        f.store
            .poll_dispatch(&data::token(), &poll())
            .await
            .expect("claim");
        let mut report = data::report();
        let command = view.command;
        report.events[0].command = command.clone();
        report.events[0].outcome = CommandOutcome::Completed {
            event: Box::new(orchestrator_protocol::agent::AgentEvent {
                version: command.version,
                command_id: command.command_id,
                group_id: command.group_id,
                node_id: command.node_id,
                target: command.target,
                kind: AgentEventKind::Cancelled,
            }),
        };
        // Existing completed start is permitted; future writes must fail.
        if fault.contains("reject_completed") {
            f.sql("ALTER TABLE dispatches ADD CONSTRAINT reject_completed CHECK(state<>'completed') NOT VALID").await;
        } else {
            f.sql(fault).await;
        }
        assert!(f.store.sync(&data::token(), &report).await.is_err());
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM node_events")
            .fetch_one(&f.raw)
            .await
            .expect("events");
        assert_eq!(events, 0);
        f.close().await;
    }
}
