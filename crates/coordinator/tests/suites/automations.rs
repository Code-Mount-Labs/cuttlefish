use crate::tests::{
    common::Fixture,
    routing::{id, ready},
};
use crate::{
    automations::{ScheduledRoute, Workflow},
    store::Store,
};
use orchestrator_protocol::execution::{LocalProvider, ProviderWorkspace};
use uuid::Uuid;

fn workflow() -> Workflow {
    Workflow {
        id: Uuid::from_u128(100),
        revision: 0,
        name: "Weekly review".into(),
        workspace_id: Uuid::from_u128(5),
        instructions: "Review the project and write a summary.".into(),
    }
}
fn route() -> ScheduledRoute {
    ScheduledRoute {
        id: Uuid::from_u128(101),
        revision: 0,
        name: "Morning review".into(),
        workflow_id: workflow().id,
        enabled: true,
        first_run_ms: 0,
        interval_minutes: 60,
    }
}
#[tokio::test]
async fn schedules_coalesce_do_not_overlap_and_recover_the_exact_run_after_restart() {
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
        .save_workflow(&token, workflow())
        .await
        .expect("workflow");
    assert!(f.store.save_workflow(&token, workflow()).await.is_err());
    let mut edited = workflow();
    edited.revision = 1;
    edited.name = "Updated".into();
    f.store
        .save_workflow(&token, edited.clone())
        .await
        .expect("update");
    assert!(f.store.save_workflow(&token, edited).await.is_err());
    f.store.save_route(&token, route()).await.expect("route");
    assert!(f.store.save_route(&token, route()).await.is_err());
    let (a, b) = tokio::join!(f.store.schedule_due(&token), f.store.schedule_due(&token));
    a.expect("first scheduler");
    b.expect("second scheduler");
    let pending = f.store.pending_workflows(&token).await.expect("pending");
    assert_eq!(pending.len(), 1);
    let original = pending[0].clone();
    f.store
        .acknowledge_workflow(&token, Uuid::from(original.command_id), false)
        .await
        .expect("retry");
    let reopened = Store::open(&f.url).await.expect("restart");
    assert_eq!(
        reopened.pending_workflows(&token).await.expect("recovered"),
        pending
    );
    let mut rescheduled = route();
    rescheduled.revision = 1;
    rescheduled.first_run_ms = 1;
    reopened
        .save_route(&token, rescheduled.clone())
        .await
        .expect("reschedule");
    reopened.schedule_due(&token).await.expect("skip overlap");
    assert_eq!(
        reopened.pending_workflows(&token).await.expect("one run"),
        pending
    );
    rescheduled.revision = 2;
    rescheduled.enabled = false;
    reopened
        .save_route(&token, rescheduled.clone())
        .await
        .expect("pause");
    reopened.schedule_due(&token).await.expect("paused");
    let data = reopened.automations(&token).await.expect("list");
    assert_eq!(data["routes"][0]["revision"], 3);
    assert_eq!(data["routes"][0]["enabled"], false);
    assert!(
        data["runs"][0]["error"]
            .as_str()
            .expect("retry status")
            .contains("Waiting")
    );
    check_manual_runs(&reopened, &token).await;
    let outsider = "f".repeat(64).try_into().expect("token");
    reopened
        .enroll(id(90), id(91), &outsider)
        .await
        .expect("other group");
    assert_eq!(
        reopened.automations(&outsider).await.expect("isolated")["workflows"],
        serde_json::json!([])
    );
    assert!(
        reopened
            .run_workflow(&outsider, workflow().id, Uuid::new_v4())
            .await
            .is_err()
    );
    assert!(reopened.save_route(&outsider, route()).await.is_err());
    check_validation(&reopened, &token).await;
    reopened.close().await;
    f.close().await;
}

async fn check_manual_runs(reopened: &Store, token: &orchestrator_protocol::sync::NodeToken) {
    let run = Uuid::from_u128(200);
    let (a, b) = tokio::join!(
        reopened.run_workflow(token, workflow().id, run),
        reopened.run_workflow(token, workflow().id, run)
    );
    a.expect("run");
    b.expect("idempotent run");
    assert_eq!(
        reopened
            .pending_workflows(token)
            .await
            .expect("manual plus scheduled")
            .len(),
        2
    );
    assert!(
        reopened
            .run_workflow(token, Uuid::from_u128(300), run)
            .await
            .is_err()
    );
    reopened
        .acknowledge_workflow(token, run, true)
        .await
        .expect("ack");
    reopened
        .acknowledge_workflow(token, run, false)
        .await
        .expect("late failure ignored");
    assert_eq!(
        reopened
            .pending_workflows(token)
            .await
            .expect("ack retained")
            .len(),
        1
    );
}

async fn check_validation(reopened: &Store, token: &orchestrator_protocol::sync::NodeToken) {
    for bad in [
        Workflow {
            id: Uuid::nil(),
            ..workflow()
        },
        Workflow {
            revision: -1,
            ..workflow()
        },
        Workflow {
            name: " ".into(),
            ..workflow()
        },
        Workflow {
            name: "x".repeat(129),
            ..workflow()
        },
        Workflow {
            instructions: " ".into(),
            ..workflow()
        },
        Workflow {
            workspace_id: Uuid::new_v4(),
            ..workflow()
        },
    ] {
        assert!(reopened.save_workflow(token, bad).await.is_err());
    }
    for bad in [
        ScheduledRoute {
            id: Uuid::nil(),
            ..route()
        },
        ScheduledRoute {
            interval_minutes: 0,
            ..route()
        },
        ScheduledRoute {
            interval_minutes: 525_601,
            ..route()
        },
        ScheduledRoute {
            first_run_ms: -1,
            ..route()
        },
        ScheduledRoute {
            first_run_ms: i64::MAX,
            ..route()
        },
        ScheduledRoute {
            revision: 50,
            ..route()
        },
    ] {
        assert!(reopened.save_route(token, bad).await.is_err());
    }
    assert!(
        reopened
            .run_workflow(token, workflow().id, Uuid::nil())
            .await
            .is_err()
    );
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    CreateWorkflow,
    UpdateWorkflow,
    CreateRoute,
    UpdateRoute,
    Run,
    Schedule,
    List,
    Pending,
    Acknowledge,
}
async fn exercise(
    store: &Store,
    token: &orchestrator_protocol::sync::NodeToken,
    operation: Operation,
) -> Result<(), crate::Error> {
    match operation {
        Operation::CreateWorkflow => {
            store
                .save_workflow(
                    token,
                    Workflow {
                        id: Uuid::from_u128(300),
                        ..workflow()
                    },
                )
                .await
        }
        Operation::UpdateWorkflow => {
            store
                .save_workflow(
                    token,
                    Workflow {
                        revision: 1,
                        ..workflow()
                    },
                )
                .await
        }
        Operation::CreateRoute => {
            store
                .save_route(
                    token,
                    ScheduledRoute {
                        id: Uuid::from_u128(301),
                        ..route()
                    },
                )
                .await
        }
        Operation::UpdateRoute => {
            store
                .save_route(
                    token,
                    ScheduledRoute {
                        revision: 1,
                        ..route()
                    },
                )
                .await
        }
        Operation::Run => {
            store
                .run_workflow(token, workflow().id, Uuid::from_u128(302))
                .await
        }
        Operation::Schedule => store.schedule_due(token).await,
        Operation::List => store.automations(token).await.map(|_| ()),
        Operation::Pending => store.pending_workflows(token).await.map(|_| ()),
        Operation::Acknowledge => {
            store
                .acknowledge_workflow(token, Uuid::from_u128(303), true)
                .await
        }
    }
}
async fn prepared() -> (Fixture, orchestrator_protocol::sync::NodeToken) {
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
        .save_workflow(&token, workflow())
        .await
        .expect("workflow");
    f.store.save_route(&token, route()).await.expect("route");
    f.store
        .run_workflow(&token, workflow().id, Uuid::from_u128(303))
        .await
        .expect("run");
    (f, token)
}
#[tokio::test]
async fn automation_authentication_and_closed_storage_fail_explicitly() {
    let (f, token) = prepared().await;
    let bad = "e".repeat(64).try_into().expect("token");
    for operation in [
        Operation::CreateWorkflow,
        Operation::UpdateWorkflow,
        Operation::CreateRoute,
        Operation::UpdateRoute,
        Operation::Run,
        Operation::Schedule,
        Operation::List,
        Operation::Pending,
        Operation::Acknowledge,
    ] {
        assert!(exercise(&f.store, &bad, operation).await.is_err());
    }
    f.store.close().await;
    for operation in [
        Operation::CreateWorkflow,
        Operation::UpdateWorkflow,
        Operation::CreateRoute,
        Operation::UpdateRoute,
        Operation::Run,
        Operation::Schedule,
        Operation::List,
        Operation::Pending,
        Operation::Acknowledge,
    ] {
        assert!(exercise(&f.store, &token, operation).await.is_err());
    }
    f.close().await;
}
#[tokio::test]
async fn automation_statement_failures_are_reported_without_partial_commits() {
    for (operation, fault) in [
        (
            Operation::List,
            "ALTER TABLE workflows RENAME COLUMN definition TO hidden",
        ),
        (
            Operation::List,
            "ALTER TABLE scheduled_routes RENAME COLUMN definition TO hidden",
        ),
        (
            Operation::List,
            "ALTER TABLE workflow_runs RENAME COLUMN error TO hidden",
        ),
        (
            Operation::CreateWorkflow,
            "ALTER TABLE provider_workspaces RENAME COLUMN workspace_id TO hidden",
        ),
        (
            Operation::CreateWorkflow,
            "ALTER TABLE workflows ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::UpdateWorkflow,
            "ALTER TABLE workflows ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::CreateRoute,
            "ALTER TABLE scheduled_routes ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::UpdateRoute,
            "ALTER TABLE scheduled_routes ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Run,
            "ALTER TABLE workflow_runs RENAME COLUMN workflow_id TO hidden",
        ),
        (
            Operation::Run,
            "ALTER TABLE workflows RENAME COLUMN definition TO hidden",
        ),
        (
            Operation::Run,
            "UPDATE provider_workspaces SET workspace=jsonb_set(workspace,'{provider}','null')",
        ),
        (
            Operation::Run,
            "ALTER TABLE workflow_runs ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE scheduled_routes RENAME COLUMN next_run_ms TO hidden",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE workflow_runs RENAME COLUMN route_id TO hidden",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE workflow_runs ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Schedule,
            "ALTER TABLE scheduled_routes ADD CONSTRAINT reject_write CHECK(FALSE) NOT VALID",
        ),
        (
            Operation::Pending,
            "ALTER TABLE workflow_runs RENAME COLUMN request TO hidden",
        ),
        (
            Operation::Acknowledge,
            "ALTER TABLE workflow_runs RENAME COLUMN error TO hidden",
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
async fn automation_commit_failure_preserves_definitions_and_pending_intents() {
    for (operation, table, event) in [
        (Operation::CreateWorkflow, "workflows", "INSERT"),
        (Operation::CreateRoute, "scheduled_routes", "INSERT"),
        (Operation::Run, "workflow_runs", "INSERT"),
        (Operation::Schedule, "scheduled_routes", "UPDATE"),
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
#[tokio::test]
async fn concurrent_run_identity_cannot_be_reused_for_a_different_workflow() {
    let (f, token) = prepared().await;
    let other = Workflow {
        id: Uuid::from_u128(400),
        ..workflow()
    };
    f.store
        .save_workflow(&token, other.clone())
        .await
        .expect("other workflow");
    f.sql("CREATE FUNCTION delay_insert() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN PERFORM pg_advisory_xact_lock(818401,401); RETURN NEW; END'; CREATE TRIGGER delay_insert BEFORE INSERT ON workflow_runs FOR EACH ROW EXECUTE FUNCTION delay_insert()").await;
    let other_token = ready(&f, 3, 5).await;
    let run = Uuid::from_u128(401);
    let mut gate = f.raw.begin().await.expect("barrier transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(818401,401)")
        .execute(&mut *gate)
        .await
        .expect("hold insertion barrier");
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND classid=818401 AND objid=401 AND NOT granted")
                    .fetch_one(&f.raw).await.expect("waiting inserts");
                if waiting == 2 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("both requests reach insertion before either commits");
        gate.commit().await.expect("release insertion barrier");
    };
    let (a, b, ()) = tokio::join!(
        f.store.run_workflow(&token, workflow().id, run),
        f.store.run_workflow(&other_token, other.id, run),
        release
    );
    assert_ne!(a.is_ok(), b.is_ok());
    f.close().await;
}
