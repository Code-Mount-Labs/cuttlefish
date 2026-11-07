use crate::tests::{common::Fixture, data};
use orchestrator_coordinator::{Error, store::Store};
use orchestrator_protocol::sync::{CommandOutcome, NodeToken};

#[tokio::test]
async fn replicas_share_identity_presence_events_and_revocation() {
    let f = Fixture::enrolled().await;
    let second = Store::open(&f.url).await.expect("second replica");
    let report = data::report();
    let token = data::token();
    let unknown = NodeToken::try_from("b".repeat(64)).expect("token");
    assert!(matches!(
        f.store.sync(&unknown, &report).await,
        Err(Error::Unauthorized)
    ));
    let before = f.store.nodes(&token).await.expect("nodes");
    assert_eq!(before.len(), 1);
    assert!(before[0].profile.is_none());
    let (a, b) = tokio::join!(f.store.sync(&token, &report), second.sync(&token, &report));
    assert_eq!(a.expect("first").acknowledged, vec![1]);
    assert_eq!(b.expect("concurrent repeat").acknowledged, vec![1]);
    let events = second.events(&token, 0).await.expect("durable event");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, report.events[0]);
    assert!(
        second
            .events(&token, events[0].cursor)
            .await
            .expect("next page")
            .is_empty()
    );
    assert!(second.events(&token, -1).await.is_err());
    let nodes = second.nodes(&token).await.expect("presence");
    assert_eq!(nodes[0].profile.as_ref(), Some(&report.profile));
    assert!(nodes[0].last_seen_ms.is_some());
    let hash: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM nodes")
        .fetch_one(&f.raw)
        .await
        .expect("hash");
    assert_eq!(hash.len(), 32);
    assert_ne!(hash, token.expose().as_bytes());
    assert!(
        second
            .enroll(report.group_id, report.node_id, &token)
            .await
            .is_err()
    );
    assert!(
        second
            .revoke(report.group_id, report.node_id)
            .await
            .expect("revoke")
    );
    assert!(
        second
            .revoke(report.group_id, report.node_id)
            .await
            .expect("repeat revoke")
    );
    assert!(matches!(
        second.nodes(&token).await,
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        second.events(&token, 0).await,
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        second.sync(&token, &report).await,
        Err(Error::Unauthorized)
    ));
    let absent = "00000000-0000-0000-0000-000000000099".parse().expect("id");
    assert!(
        !second
            .revoke(report.group_id, absent)
            .await
            .expect("unknown")
    );
    second.close().await;
    f.close().await;
}

#[tokio::test]
async fn groups_are_isolated_and_batches_are_atomic_on_conflict() {
    let f = Fixture::enrolled().await;
    let token = data::token();
    let mut report = data::report();
    f.store.sync(&token, &report).await.expect("first");
    let original = f.store.events(&token, 0).await.expect("events");
    let mut duplicate_command = report.clone();
    duplicate_command.events[0].sequence = 2;
    assert!(matches!(
        f.store.sync(&token, &duplicate_command).await,
        Err(Error::Conflict)
    ));
    let mut new_event = report.events[0].clone();
    new_event.sequence = 2;
    new_event.command.command_id = "00000000-0000-0000-0000-000000000088".parse().expect("id");
    new_event.outcome = CommandOutcome::Rejected {
        reason: "declined".into(),
    };
    report.events[0].outcome = CommandOutcome::Rejected {
        reason: "changed historical result".into(),
    };
    report.events.insert(0, new_event);
    assert!(matches!(
        f.store.sync(&token, &report).await,
        Err(Error::Conflict)
    ));
    assert_eq!(f.store.events(&token, 0).await.expect("rollback"), original);
    report = data::report();
    report.node_id = "00000000-0000-0000-0000-000000000099".parse().expect("id");
    assert!(matches!(
        f.store.sync(&token, &report).await,
        Err(Error::Unauthorized)
    ));
    report.group_id = "00000000-0000-0000-0000-000000000077".parse().expect("id");
    let other = NodeToken::try_from("b".repeat(64)).expect("token");
    f.store
        .enroll(report.group_id, report.node_id, &other)
        .await
        .expect("other tenant");
    assert!(matches!(
        f.store.sync(&other, &report).await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        f.store.nodes(&other).await.expect("isolated nodes").len(),
        1
    );
    assert!(
        f.store
            .events(&other, 0)
            .await
            .expect("isolated events")
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn closed_storage_and_corrupt_migrations_fail_explicitly() {
    assert!(Store::open("not a database URL").await.is_err());
    let f = Fixture::enrolled().await;
    f.sql("UPDATE _sqlx_migrations SET checksum='broken'::bytea")
        .await;
    assert!(Store::open(&f.url).await.is_err());
    f.store.close().await;
    let r = data::report();
    let t = data::token();
    assert!(f.store.enroll(r.group_id, r.node_id, &t).await.is_err());
    assert!(f.store.revoke(r.group_id, r.node_id).await.is_err());
    assert!(f.store.sync(&t, &r).await.is_err());
    assert!(f.store.nodes(&t).await.is_err());
    assert!(f.store.events(&t, 0).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn sync_statement_and_commit_failures_never_acknowledge_or_partially_write() {
    for sql in [
        "DROP TABLE nodes CASCADE;",
        "DROP TABLE groups CASCADE;",
        "ALTER TABLE node_events ADD CONSTRAINT reject_events CHECK (local_sequence < 0);",
        "ALTER TABLE nodes ADD CONSTRAINT reject_presence CHECK (last_seen IS NULL);",
        "CREATE TABLE fault_parent(id INT PRIMARY KEY); CREATE TABLE fault_child(id INT REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE FUNCTION fail_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN INSERT INTO fault_child VALUES(1); RETURN NEW; END'; CREATE TRIGGER fail_commit AFTER INSERT ON node_events FOR EACH ROW EXECUTE FUNCTION fail_commit();",
    ] {
        let f = Fixture::enrolled().await;
        f.sql(sql).await;
        assert!(f.store.sync(&data::token(), &data::report()).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM node_events")
            .fetch_one(&f.raw)
            .await
            .expect("event count");
        assert_eq!(count, 0);
        f.close().await;
    }
}

#[tokio::test]
async fn enrollment_is_atomic_and_read_errors_propagate() {
    for sql in [
        "ALTER TABLE groups ADD CONSTRAINT reject_groups CHECK (FALSE);",
        "ALTER TABLE nodes ADD CONSTRAINT reject_nodes CHECK (FALSE);",
        "CREATE TABLE fault_parent(id INT PRIMARY KEY); CREATE TABLE fault_child(id INT REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE FUNCTION fail_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN INSERT INTO fault_child VALUES(1); RETURN NEW; END'; CREATE TRIGGER fail_commit AFTER INSERT ON nodes FOR EACH ROW EXECUTE FUNCTION fail_commit();",
    ] {
        let f = Fixture::new().await;
        f.sql(sql).await;
        let r = data::report();
        assert!(
            f.store
                .enroll(r.group_id, r.node_id, &data::token())
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM groups")
            .fetch_one(&f.raw)
            .await
            .expect("count");
        assert_eq!(count, 0);
        f.close().await;
    }
    let f = Fixture::enrolled().await;
    f.sql("UPDATE nodes SET profile='null'::jsonb").await;
    assert!(f.store.nodes(&data::token()).await.is_err());
    f.sql("UPDATE nodes SET profile=NULL,node_id='00000000-0000-0000-0000-000000000000'")
        .await;
    assert!(f.store.nodes(&data::token()).await.is_err());
    f.sql("DROP TABLE node_events").await;
    assert!(f.store.events(&data::token(), 0).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn credential_identity_is_exact_revocable_and_rejects_corruption() {
    let f = Fixture::enrolled().await;
    let report = data::report();
    let token = data::token();
    assert_eq!(
        f.store.identity(&token).await.expect("identity"),
        (report.group_id, report.node_id)
    );
    assert!(
        f.store
            .identity(&NodeToken::try_from("b".repeat(64)).expect("unknown"))
            .await
            .is_err()
    );
    f.sql("INSERT INTO groups(group_id) VALUES('00000000-0000-0000-0000-000000000000'); UPDATE nodes SET group_id='00000000-0000-0000-0000-000000000000'").await;
    assert!(f.store.identity(&token).await.is_err());
    sqlx::query("UPDATE nodes SET group_id=$1,node_id='00000000-0000-0000-0000-000000000000'")
        .bind(uuid::Uuid::from(report.group_id))
        .execute(&f.raw)
        .await
        .expect("corrupt node");
    assert!(f.store.identity(&token).await.is_err());
    sqlx::query("UPDATE nodes SET node_id=$1")
        .bind(uuid::Uuid::from(report.node_id))
        .execute(&f.raw)
        .await
        .expect("restore");
    f.store
        .revoke(report.group_id, report.node_id)
        .await
        .expect("revoke");
    assert!(f.store.identity(&token).await.is_err());
    f.store.close().await;
    assert!(f.store.identity(&token).await.is_err());
    f.close().await;
}
