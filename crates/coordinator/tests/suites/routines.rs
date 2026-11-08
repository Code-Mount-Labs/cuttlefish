use crate::tests::{
    common::Fixture,
    routing::{id, ready},
};
use crate::{
    automations::{Routine, ScheduledRoute, Workflow},
    store::Store,
};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    routing::RouteAction,
    sync::NodeToken,
};
use uuid::Uuid;

fn routine() -> Routine {
    Routine {
        id: Uuid::from_u128(800),
        revision: 0,
        name: "Daily news".into(),
        workspace_id: Uuid::from_u128(5),
        instructions: "Summarize today's news.".into(),
        enabled: true,
        first_run_ms: 0,
        interval_minutes: 60,
    }
}
async fn prepared() -> (Fixture, NodeToken) {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 5).await;
    f.store
        .advertise_workspaces(
            &token,
            &[ProviderWorkspace {
                workspace_id: id(5),
                name: "Workspace".into(),
                provider: LocalProvider::Codex,
                allow_edits: true,
            }],
        )
        .await
        .expect("workspace");
    f.store
        .save_routine(&token, routine())
        .await
        .expect("routine without workflow");
    (f, token)
}

#[tokio::test]
async fn routines_own_prompts_and_recover_scheduled_and_manual_requests() {
    let (f, token) = prepared().await;
    assert!(f.store.save_routine(&token, routine()).await.is_err());
    assert_eq!(
        f.store.automations(&token).await.expect("list")["workflows"],
        serde_json::json!([])
    );
    let (a, b) = tokio::join!(f.store.schedule_due(&token), f.store.schedule_due(&token));
    a.expect("scheduler");
    b.expect("second scheduler");
    let pending = f.store.pending_workflows(&token).await.expect("scheduled");
    assert_eq!(pending.len(), 1);
    let RouteAction::Start {
        message,
        workspace_id,
        ..
    } = &pending[0].action
    else {
        unreachable!("start expected")
    };
    assert_eq!(message.text.as_str(), routine().instructions);
    assert_eq!(*workspace_id, id(5));
    let mut edited = routine();
    edited.revision = 1;
    edited.instructions = "Updated instructions".into();
    edited.first_run_ms = 1;
    f.store
        .save_routine(&token, edited.clone())
        .await
        .expect("edit");
    assert!(f.store.save_routine(&token, edited.clone()).await.is_err());
    let reopened = Store::open(&f.url).await.expect("restart");
    reopened
        .schedule_due(&token)
        .await
        .expect("coalesce active run");
    assert_eq!(
        reopened
            .pending_workflows(&token)
            .await
            .expect("snapshot unchanged"),
        pending
    );
    edited.revision = 2;
    edited.enabled = false;
    reopened.save_routine(&token, edited).await.expect("pause");
    reopened.schedule_due(&token).await.expect("paused");
    let run = Uuid::from_u128(801);
    let (a, b) = tokio::join!(
        reopened.run_routine(&token, routine().id, run),
        reopened.run_routine(&token, routine().id, run)
    );
    a.expect("manual while paused");
    b.expect("same manual request");
    let all = reopened
        .pending_workflows(&token)
        .await
        .expect("manual snapshot");
    assert_eq!(all.len(), 2);
    let manual = all
        .iter()
        .find(|r| Uuid::from(r.command_id) == run)
        .expect("manual");
    let RouteAction::Start { message, .. } = &manual.action else {
        unreachable!("start expected")
    };
    assert_eq!(message.text.as_str(), "Updated instructions");
    assert!(
        reopened
            .run_routine(&token, Uuid::from_u128(999), run)
            .await
            .is_err()
    );
    assert!(
        reopened
            .run_workflow(&token, routine().id, run)
            .await
            .is_err()
    );
    let data = reopened
        .automations(&token)
        .await
        .expect("routines and runs");
    assert_eq!(data["routines"][0]["revision"], 3);
    assert_eq!(data["routines"][0]["enabled"], false);
    assert!(
        data["routines"][0]["next_run_ms"]
            .as_i64()
            .expect("next run")
            > 1
    );
    for run in data["runs"].as_array().expect("runs") {
        assert_eq!(run["routine_id"], routine().id.to_string());
        assert!(run["workflow_id"].is_null());
    }
    verify_group_isolation(&reopened).await;
    reopened.close().await;
    f.close().await;
}

#[tokio::test]
async fn routine_validation_rejects_invalid_inputs_without_losing_saved_state() {
    let (f, token) = prepared().await;
    let before = f.store.automations(&token).await.expect("before");
    for invalid in [
        Routine {
            id: Uuid::nil(),
            ..routine()
        },
        Routine {
            revision: -1,
            ..routine()
        },
        Routine {
            name: " ".into(),
            ..routine()
        },
        Routine {
            name: "x".repeat(129),
            ..routine()
        },
        Routine {
            instructions: " ".into(),
            ..routine()
        },
        Routine {
            workspace_id: Uuid::new_v4(),
            ..routine()
        },
        Routine {
            interval_minutes: 0,
            ..routine()
        },
        Routine {
            interval_minutes: 525_601,
            ..routine()
        },
        Routine {
            first_run_ms: -1,
            ..routine()
        },
        Routine {
            first_run_ms: i64::MAX,
            ..routine()
        },
        Routine {
            revision: 99,
            ..routine()
        },
    ] {
        assert!(f.store.save_routine(&token, invalid).await.is_err());
    }
    assert!(
        f.store
            .run_routine(&token, routine().id, Uuid::nil())
            .await
            .is_err()
    );
    assert!(
        f.store
            .run_routine(&token, Uuid::new_v4(), Uuid::new_v4())
            .await
            .is_err()
    );
    assert_eq!(
        f.store.automations(&token).await.expect("unchanged"),
        before
    );
    f.close().await;
}

#[tokio::test]
async fn legacy_route_migration_preserves_due_time_and_pending_command_without_double_execution() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 5).await;
    f.store
        .advertise_workspaces(
            &token,
            &[ProviderWorkspace {
                workspace_id: id(5),
                name: "Workspace".into(),
                provider: LocalProvider::Codex,
                allow_edits: true,
            }],
        )
        .await
        .expect("workspace");
    let workflow = Workflow {
        id: Uuid::from_u128(900),
        revision: 0,
        name: "Legacy task".into(),
        workspace_id: Uuid::from_u128(5),
        instructions: "Original instructions".into(),
    };
    f.store
        .save_workflow(&token, workflow.clone())
        .await
        .expect("workflow");
    let route = ScheduledRoute {
        id: Uuid::from_u128(901),
        revision: 0,
        name: "Legacy schedule".into(),
        workflow_id: workflow.id,
        enabled: true,
        first_run_ms: 0,
        interval_minutes: 60,
    };
    f.store
        .save_route(&token, route.clone())
        .await
        .expect("legacy route");
    f.store.schedule_due(&token).await.expect("pending run");
    let pending = f.store.pending_workflows(&token).await.expect("before");
    let before = f.store.automations(&token).await.expect("before");
    // Reconstruct the previous schema exactly at the migration boundary.
    f.sql("ALTER TABLE workflow_runs DROP COLUMN routine_id CASCADE; ALTER TABLE workflow_runs ALTER COLUMN workflow_id SET NOT NULL; DROP TABLE routines; DELETE FROM _sqlx_migrations WHERE version=9").await;
    let upgraded = Store::open(&f.url)
        .await
        .expect("upgrade with pending work");
    let after = upgraded.automations(&token).await.expect("after");
    assert!(
        upgraded.save_route(&token, route.clone()).await.is_err(),
        "replaying a pre-upgrade create cannot recreate the old schedule"
    );
    assert_eq!(after["routes"], serde_json::json!([]));
    assert_eq!(after["routines"][0]["id"], route.id.to_string());
    assert_eq!(
        after["routines"][0]["next_run_ms"],
        before["routes"][0]["next_run_ms"]
    );
    assert_eq!(after["routines"][0]["instructions"], workflow.instructions);
    assert_eq!(after["runs"][0]["routine_id"], route.id.to_string());
    assert!(after["runs"][0]["workflow_id"].is_null());
    assert_eq!(
        upgraded
            .pending_workflows(&token)
            .await
            .expect("same command"),
        pending
    );
    upgraded.schedule_due(&token).await.expect("no duplicate");
    assert_eq!(
        upgraded
            .pending_workflows(&token)
            .await
            .expect("one command"),
        pending
    );
    let edited = Workflow {
        revision: 1,
        instructions: "Changed reusable workflow".into(),
        ..workflow
    };
    upgraded
        .save_workflow(&token, edited)
        .await
        .expect("independent workflow");
    assert_eq!(
        upgraded
            .automations(&token)
            .await
            .expect("independent prompt")["routines"][0]["instructions"],
        "Original instructions"
    );
    upgraded.close().await;
    f.close().await;
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Create,
    Update,
    Run,
    Schedule,
    List,
}
async fn exercise(
    store: &Store,
    token: &NodeToken,
    operation: Operation,
) -> Result<(), crate::Error> {
    match operation {
        Operation::Create => {
            store
                .save_routine(
                    token,
                    Routine {
                        id: Uuid::from_u128(810),
                        ..routine()
                    },
                )
                .await
        }
        Operation::Update => {
            store
                .save_routine(
                    token,
                    Routine {
                        revision: 1,
                        ..routine()
                    },
                )
                .await
        }
        Operation::Run => {
            store
                .run_routine(token, routine().id, Uuid::from_u128(811))
                .await
        }
        Operation::Schedule => store.schedule_due(token).await,
        Operation::List => store.automations(token).await.map(|_| ()),
    }
}
#[tokio::test]
async fn routine_authentication_and_closed_storage_fail_explicitly() {
    let (f, token) = prepared().await;
    let bad = "e".repeat(64).try_into().expect("token");
    for operation in [
        Operation::Create,
        Operation::Update,
        Operation::Run,
        Operation::Schedule,
        Operation::List,
    ] {
        assert!(exercise(&f.store, &bad, operation).await.is_err());
    }
    f.store.close().await;
    for operation in [
        Operation::Create,
        Operation::Update,
        Operation::Run,
        Operation::Schedule,
        Operation::List,
    ] {
        assert!(exercise(&f.store, &token, operation).await.is_err());
    }
    f.close().await;
}
#[tokio::test]
async fn routine_statement_failures_preserve_atomicity() {
    for (operation, fault) in [
        (
            Operation::List,
            "ALTER TABLE routines RENAME COLUMN definition TO hidden",
        ),
        (
            Operation::Create,
            "ALTER TABLE provider_workspaces RENAME COLUMN workspace_id TO hidden",
        ),
        (
            Operation::Create,
            "ALTER TABLE routines ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Update,
            "ALTER TABLE routines ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Run,
            "ALTER TABLE routines RENAME COLUMN definition TO hidden",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE routines RENAME COLUMN next_run_ms TO hidden",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE workflow_runs RENAME COLUMN routine_id TO hidden",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE workflow_runs ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE routines ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
    ] {
        let (f, token) = prepared().await;
        f.sql(fault).await;
        assert!(
            exercise(&f.store, &token, operation).await.is_err(),
            "{operation:?}: {fault}"
        );
        f.close().await;
    }
}
#[tokio::test]
async fn routine_commit_failures_do_not_persist_partial_changes() {
    for (operation, table, event) in [
        (Operation::Create, "routines", "INSERT"),
        (Operation::Update, "routines", "UPDATE"),
        (Operation::Run, "workflow_runs", "INSERT"),
        (Operation::Schedule, "routines", "UPDATE"),
    ] {
        let (f, token) = prepared().await;
        f.sql("CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'").await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE CONSTRAINT TRIGGER reject_commit AFTER {event} ON {table} DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()"))).execute(&f.raw).await.expect("trigger");
        let before = f.store.automations(&token).await.expect("before");
        assert!(exercise(&f.store, &token, operation).await.is_err());
        assert_eq!(f.store.automations(&token).await.expect("rollback"), before);
        f.close().await;
    }
}

async fn verify_group_isolation(store: &Store) {
    let outsider = "f".repeat(64).try_into().expect("token");
    store
        .enroll(id(90), id(91), &outsider)
        .await
        .expect("group");
    assert_eq!(
        store.automations(&outsider).await.expect("isolated")["routines"],
        serde_json::json!([])
    );
    assert!(
        store
            .run_routine(&outsider, routine().id, Uuid::new_v4())
            .await
            .is_err()
    );
    assert!(store.save_routine(&outsider, routine()).await.is_err());
}

#[tokio::test]
async fn routine_results_release_the_exact_worker_after_router_restart() {
    use crate::tests::{
        data,
        routing::{Selection, finish, poll},
    };
    use crate::{routing::RoutingService, selector::Unconfigured};
    use orchestrator_protocol::capabilities::Capability;
    use std::sync::Arc;
    let (f, token) = prepared().await;
    let mut report = data::report();
    report.events.clear();
    report
        .profile
        .capabilities
        .insert(Capability::local_provider(LocalProvider::Codex));
    f.store.sync(&token, &report).await.expect("capability");
    f.store
        .run_routine(&token, routine().id, Uuid::from_u128(820))
        .await
        .expect("manual run");
    let pending = f.store.pending_workflows(&token).await.expect("pending");
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let start = service
        .submit(&token, pending[0].clone())
        .await
        .expect("start");
    f.store
        .acknowledge_workflow(&token, Uuid::from(start.request.command_id), true)
        .await
        .expect("ack");
    f.store.poll_dispatch(&token, &poll()).await.expect("claim");
    finish(&f, &token, &start, 1).await;
    let cleanup = f.store.pending_workflows(&token).await.expect("cleanup");
    assert_eq!(cleanup.len(), 1);
    assert_eq!(cleanup[0].action, RouteAction::Cancel {});
    let reopened = Store::open(&f.url).await.expect("restart");
    assert_eq!(
        reopened
            .pending_workflows(&token)
            .await
            .expect("durable close"),
        cleanup
    );
    let service = RoutingService::new(reopened.clone(), Arc::new(Unconfigured));
    let cancel = service
        .submit(&token, cleanup[0].clone())
        .await
        .expect("no model needed");
    assert_eq!(cancel.command.target, start.command.target);
    reopened
        .acknowledge_workflow(&token, Uuid::from(cancel.request.command_id), true)
        .await
        .expect("ack close");
    reopened
        .poll_dispatch(&token, &poll())
        .await
        .expect("claim close");
    finish(&f, &token, &cancel, 2).await;
    assert!(
        reopened
            .pending_workflows(&token)
            .await
            .expect("released")
            .is_empty()
    );
    assert_eq!(
        reopened.automations(&token).await.expect("result retained")["runs"][0]["state"],
        "completed"
    );
    reopened.close().await;
    f.close().await;
}
