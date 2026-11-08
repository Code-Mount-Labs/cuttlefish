#![allow(clippy::expect_used)]
use super::*;
#[tokio::test]
async fn child_intents_reject_invalid_operations_and_inconsistent_goals() {
    use crate::tests::{
        agent_tools::prepared,
        routing::{id, poll},
    };
    let (f, a, _, service, parent) = prepared().await;
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    let mut request = ToolRequest {
        parent: parent.command.command_id,
        id: id(900),
        action: ToolAction::Agents {},
    };
    let thread = parent.request.thread_id;
    let goal = thread.collaboration_goal();
    assert!(
        f.store
            .prepare_child(&a, &request, thread, goal)
            .await
            .is_err()
    );
    request.action = ToolAction::Delegate {
        workspace_id: id(15),
        message: "Work".to_owned().try_into().expect("text"),
        acceptance: "Evidence".to_owned().try_into().expect("text"),
    };
    service.agent_tool(&a, &request).await.expect("child");
    request.id = id(901);
    f.sql("UPDATE goals SET snapshot=jsonb_set(snapshot,'{owner}','\"00000000-0000-0000-0000-000000000009\"')").await;
    assert!(service.agent_tool(&a, &request).await.is_err());
    f.sql("UPDATE goals SET snapshot=jsonb_set(jsonb_set(snapshot,'{owner}','\"00000000-0000-0000-0000-000000000002\"'),'{spec,thread_id}','\"00000000-0000-0000-0000-000000000099\"')").await;
    assert!(service.agent_tool(&a, &request).await.is_err());
    f.sql("UPDATE dispatches SET request=jsonb_set(request,'{action,message,text}','\"\"') WHERE command_id='00000000-0000-0000-0000-0000000000c8'").await;
    assert!(service.agent_tool(&a, &request).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn storage_and_policy_failures_do_not_commit_partial_delegations() {
    use crate::tests::{
        agent_tools::prepared,
        routing::{id, poll},
    };
    for fault in [
        "ALTER TABLE groups RENAME COLUMN group_id TO hidden",
        "UPDATE provider_workspaces SET workspace='{}' WHERE workspace_id='00000000-0000-0000-0000-00000000000f'",
        "ALTER TABLE dispatches RENAME COLUMN request TO hidden",
        "ALTER TABLE provider_workspaces RENAME COLUMN workspace TO hidden",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail_write BEFORE INSERT ON goals FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail_write BEFORE INSERT ON goal_tasks FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail_write BEFORE UPDATE ON goals FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_write AFTER INSERT ON goal_tasks DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_write()",
    ] {
        let (f, a, _, service, parent) = prepared().await;
        f.store.poll_dispatch(&a, &poll()).await.expect("claim");
        f.sql(fault).await;
        let request = ToolRequest {
            parent: parent.command.command_id,
            id: id(900),
            action: ToolAction::Delegate {
                workspace_id: id(15),
                message: "Work".to_owned().try_into().expect("message"),
                acceptance: "Evidence".to_owned().try_into().expect("acceptance"),
            },
        };
        assert!(service.agent_tool(&a, &request).await.is_err(), "{fault}");
        f.close().await;
    }
    for revision in [u64::MAX - 1, u64::MAX] {
        let (f, a, _, service, parent) = prepared().await;
        f.store.poll_dispatch(&a, &poll()).await.expect("claim");
        let mut request = ToolRequest {
            parent: parent.command.command_id,
            id: id(900),
            action: ToolAction::Delegate {
                workspace_id: id(15),
                message: "Work".to_owned().try_into().expect("message"),
                acceptance: "Evidence".to_owned().try_into().expect("acceptance"),
            },
        };
        service.agent_tool(&a, &request).await.expect("initial");
        sqlx::query("UPDATE goals SET snapshot=jsonb_set(snapshot,'{revision}',$1)")
            .bind(json!(revision))
            .execute(&f.raw)
            .await
            .expect("boundary");
        request.id = id(901);
        assert!(service.agent_tool(&a, &request).await.is_err());
        f.close().await;
    }
}

#[tokio::test]
async fn tool_reads_and_second_transaction_failures_are_explicit() {
    use crate::tests::{
        agent_tools::prepared,
        routing::{id, poll},
    };
    for fault in [
        "ALTER TABLE nodes RENAME COLUMN profile TO hidden",
        "ALTER TABLE provider_workspaces RENAME COLUMN workspace TO hidden",
        "ALTER TABLE dispatches RENAME COLUMN thread_id TO hidden",
    ] {
        let (f, a, _, service, parent) = prepared().await;
        f.store.poll_dispatch(&a, &poll()).await.expect("claim");
        f.sql(fault).await;
        assert!(
            service
                .agent_tool(
                    &a,
                    &ToolRequest {
                        parent: parent.command.command_id,
                        id: id(999),
                        action: ToolAction::Agents {}
                    }
                )
                .await
                .is_err()
        );
        f.close().await;
    }
    let (f, a, _, service, parent) = prepared().await;
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    assert!(
        f.store
            .conversation_tasks(
                &"f".repeat(64).try_into().expect("unknown token"),
                parent.request.thread_id
            )
            .await
            .is_err()
    );
    let mut request = ToolRequest {
        parent: parent.command.command_id,
        id: id(999),
        action: ToolAction::Task { task_id: id(999) },
    };
    assert!(service.agent_tool(&a, &request).await.is_err());
    request.action = ToolAction::Delegate {
        workspace_id: id(15),
        message: "Work".to_owned().try_into().expect("message"),
        acceptance: "Evidence".to_owned().try_into().expect("acceptance"),
    };
    let thread = parent.request.thread_id;
    f.sql("ALTER TABLE groups RENAME COLUMN group_id TO hidden")
        .await;
    assert!(
        f.store
            .prepare_child(&a, &request, thread, thread.collaboration_goal())
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn unavailable_capacity_retains_intent_and_corrupt_goal_rolls_back() {
    use crate::tests::{
        agent_tools::prepared,
        routing::{id, poll},
    };
    let (f, a, _, service, parent) = prepared().await;
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    let mut request = ToolRequest {
        parent: parent.command.command_id,
        id: id(999),
        action: ToolAction::Delegate {
            workspace_id: id(15),
            message: "Work".to_owned().try_into().expect("message"),
            acceptance: "Evidence".to_owned().try_into().expect("acceptance"),
        },
    };
    f.sql(
        "UPDATE nodes SET dispatch_active=8 WHERE node_id='00000000-0000-0000-0000-000000000009'",
    )
    .await;
    assert!(matches!(
        service.agent_tool(&a, &request).await,
        Err(Error::NoCapacity)
    ));
    request.action = ToolAction::Task { task_id: id(999) };
    assert!(
        service
            .agent_tool(&a, &request)
            .await
            .expect("planned task")["state"]
            .is_null()
    );
    assert_eq!(
        f.store
            .conversation_tasks(&a, parent.request.thread_id)
            .await
            .expect("intent")
            .len(),
        1
    );
    f.sql("ALTER TABLE goal_tasks RENAME COLUMN goal_id TO hidden")
        .await;
    assert!(
        f.store
            .conversation_tasks(&a, parent.request.thread_id)
            .await
            .is_err()
    );
    f.close().await;
    let (f, a, _, service, parent) = prepared().await;
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    f.sql("CREATE FUNCTION corrupt_goal() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN NEW.snapshot = ''{}''::jsonb; RETURN NEW; END'; CREATE TRIGGER corrupt BEFORE INSERT ON goals FOR EACH ROW EXECUTE FUNCTION corrupt_goal()").await;
    request.parent = parent.command.command_id;
    request.action = ToolAction::Delegate {
        workspace_id: id(15),
        message: "Work".to_owned().try_into().expect("message"),
        acceptance: "Evidence".to_owned().try_into().expect("acceptance"),
    };
    assert!(service.agent_tool(&a, &request).await.is_err());
    f.close().await;
}
