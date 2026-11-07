//! Inject real `SQLite` statement and deferred-commit failures; verify propagation
//! and transaction atomicity without production-only fault switches.
#![allow(clippy::expect_used)]

use crate::common;

use common::TestAgent as FakeAgent;
use common::{Fixture, cancel, id, identity, instance, outcome, start};
use orchestrator_node::{journal::Journal, model::CommandStatus, worker};

async fn trigger(fixture: &Fixture, target: &str, timing: &str, condition: &str, body: &str) {
    let raw = fixture.raw().await;
    let statement =
        format!("CREATE TRIGGER injected {timing} ON {target} WHEN {condition} BEGIN {body} END;");
    // All fragments are fixed test fixtures; no external input is interpolated.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
        .execute(&raw)
        .await
        .expect("failure trigger");
    raw.close().await;
}

async fn deferred_commit_failure(fixture: &Fixture, target: &str, operation: &str) {
    let raw = fixture.raw().await;
    sqlx::raw_sql("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);")
        .execute(&raw).await.expect("deferred constraint");
    raw.close().await;
    trigger(
        fixture,
        target,
        operation,
        "1",
        "INSERT INTO fault_child VALUES(123);",
    )
    .await;
}

#[tokio::test]
async fn closed_storage_errors_propagate_from_every_journal_operation() {
    let fixture = Fixture::new().await;
    fixture.journal.close().await;
    assert!(
        fixture
            .journal
            .mount(id(5), fixture.temp.path())
            .await
            .is_err()
    );
    assert!(fixture.journal.accept(&start(), instance()).await.is_err());
    assert!(fixture.journal.command(id(6)).await.is_err());
    assert!(fixture.journal.session(id(4)).await.is_err());
    assert!(fixture.journal.heartbeat(id(4), "instance").await.is_err());
    assert!(fixture.journal.claim(id(4), "instance").await.is_err());
    assert!(
        fixture
            .journal
            .complete(id(6), "instance", &outcome(&start()))
            .await
            .is_err()
    );
    assert!(
        fixture
            .journal
            .interrupted(id(4), "instance", "lost")
            .await
            .is_err()
    );
    assert!(fixture.journal.expire(0).await.is_err());
    assert!(fixture.journal.request_stop_all().await.is_err());
    assert!(fixture.journal.pending_events().await.is_err());
    assert!(fixture.journal.acknowledge(1).await.is_err());
}

#[tokio::test]
async fn initialization_handles_identity_write_and_decode_failures() {
    let fixture = Fixture::new().await;
    trigger(
        &fixture,
        "node_identity",
        "BEFORE INSERT",
        "1",
        "SELECT RAISE(ABORT,'identity write failed');",
    )
    .await;
    assert!(
        Journal::open(fixture.journal.path(), identity())
            .await
            .is_err()
    );
    let raw = fixture.raw().await;
    sqlx::raw_sql("DROP TRIGGER injected; UPDATE node_identity SET identity='null';")
        .execute(&raw)
        .await
        .expect("corrupt identity");
    assert!(
        Journal::open(fixture.journal.path(), identity())
            .await
            .is_err()
    );
    raw.close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn acceptance_fails_closed_for_missing_tables_and_insert_failures() {
    for table in ["command_receipts", "local_sessions", "workspace_mounts"] {
        let fixture = Fixture::new().await;
        let raw = fixture.raw().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
            .execute(&raw)
            .await
            .expect("remove table");
        assert!(fixture.journal.accept(&start(), instance()).await.is_err());
        raw.close().await;
        fixture.journal.close().await;
    }
    let fixture = Fixture::new().await;
    trigger(
        &fixture,
        "local_sessions",
        "BEFORE INSERT",
        "1",
        "SELECT RAISE(ABORT,'session insert failed');",
    )
    .await;
    assert!(fixture.journal.accept(&start(), instance()).await.is_err());
    fixture.journal.close().await;
}

#[tokio::test]
async fn acceptance_commit_failure_rolls_back_both_session_and_command() {
    let fixture = Fixture::new().await;
    deferred_commit_failure(&fixture, "command_receipts", "AFTER INSERT").await;
    assert!(fixture.journal.accept(&start(), instance()).await.is_err());
    assert!(
        fixture
            .journal
            .session(id(4))
            .await
            .expect("rolled back")
            .is_none()
    );
    assert!(
        fixture
            .journal
            .command(id(6))
            .await
            .expect("rolled back")
            .is_none()
    );
    fixture.journal.close().await;
}

#[tokio::test]
async fn completion_rejects_corrupt_session_and_statement_failures() {
    for location in 0..3 {
        let fixture = Fixture::new().await;
        fixture.registered().await;
        fixture
            .journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("claim");
        if location == 0 {
            let raw = fixture.raw().await;
            sqlx::query("UPDATE local_sessions SET target='null'")
                .execute(&raw)
                .await
                .expect("corrupt record");
            raw.close().await;
        } else if location == 1 {
            trigger(
                &fixture,
                "command_receipts",
                "BEFORE UPDATE",
                "NEW.status='completed'",
                "SELECT RAISE(ABORT,'receipt write failed');",
            )
            .await;
        } else {
            deferred_commit_failure(&fixture, "pending_events", "AFTER INSERT").await;
        }
        assert!(
            fixture
                .journal
                .complete(id(6), &instance().to_string(), &outcome(&start()))
                .await
                .is_err()
        );
        assert_eq!(fixture.status(6).await, CommandStatus::Executing);
        assert!(
            fixture
                .journal
                .pending_events()
                .await
                .expect("atomic outbox")
                .is_empty()
        );
        fixture.journal.close().await;
    }
}

#[tokio::test]
async fn cancellation_rolls_back_if_session_or_pending_command_updates_fail() {
    for table in ["local_sessions", "command_receipts"] {
        let fixture = Fixture::new().await;
        fixture.completed().await;
        fixture
            .journal
            .accept(&cancel(11), instance())
            .await
            .expect("cancel");
        fixture
            .journal
            .accept(&common::send(12), instance())
            .await
            .expect("pending message");
        fixture
            .journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("claim");
        let condition = if table == "local_sessions" {
            "NEW.state='stopped'"
        } else {
            "NEW.status='uncertain'"
        };
        trigger(
            &fixture,
            table,
            "BEFORE UPDATE",
            condition,
            "SELECT RAISE(ABORT,'cancel write failed');",
        )
        .await;
        assert!(
            fixture
                .journal
                .complete(id(11), &instance().to_string(), &outcome(&cancel(11)))
                .await
                .is_err()
        );
        assert_eq!(fixture.status(11).await, CommandStatus::Executing);
        assert_eq!(fixture.status(12).await, CommandStatus::Queued);
        fixture.journal.close().await;
    }
}

#[tokio::test]
async fn interruption_and_expiry_updates_are_atomic_even_at_commit() {
    for expire in [false, true] {
        for stage in 0..3 {
            let fixture = Fixture::new().await;
            fixture.registered().await;
            if stage == 2 {
                deferred_commit_failure(&fixture, "local_sessions", "AFTER UPDATE").await;
            } else {
                let table = if stage == 0 {
                    "local_sessions"
                } else {
                    "command_receipts"
                };
                trigger(
                    &fixture,
                    table,
                    "BEFORE UPDATE",
                    "1",
                    "SELECT RAISE(ABORT,'interruption write failed');",
                )
                .await;
            }
            let result = if expire {
                fixture.journal.expire(0).await
            } else {
                fixture
                    .journal
                    .interrupted(id(4), &instance().to_string(), "died")
                    .await
            };
            assert!(result.is_err());
            assert_eq!(
                fixture.session().await.state,
                orchestrator_node::model::SessionStatus::Running
            );
            assert_eq!(fixture.status(6).await, CommandStatus::Queued);
            fixture.journal.close().await;
        }
    }
}

#[tokio::test]
async fn worker_stops_on_disappearing_corrupt_or_unwritable_state() {
    for failure in 0..4 {
        let fixture = Fixture::new().await;
        fixture.accepted().await;
        match failure {
            0 => {
                trigger(
                    &fixture,
                    "local_sessions",
                    "AFTER UPDATE",
                    "OLD.state='starting'",
                    "DELETE FROM command_receipts; DELETE FROM local_sessions;",
                )
                .await;
            }
            1 => {
                trigger(
                    &fixture,
                    "local_sessions",
                    "AFTER UPDATE",
                    "OLD.state='starting'",
                    "UPDATE local_sessions SET target='null';",
                )
                .await;
            }
            2 => {
                trigger(
                    &fixture,
                    "local_sessions",
                    "BEFORE UPDATE",
                    "OLD.state='running'",
                    "SELECT RAISE(ABORT,'heartbeat failed');",
                )
                .await;
            }
            _ => {
                trigger(
                    &fixture,
                    "command_receipts",
                    "BEFORE UPDATE",
                    "NEW.status='executing'",
                    "SELECT RAISE(ABORT,'claim failed');",
                )
                .await;
            }
        }
        let result = worker::run(
            &fixture.journal,
            id(4),
            &instance().to_string(),
            FakeAgent::new(id(1), id(2)),
        )
        .await;
        assert!(result.is_err());
        fixture.journal.close().await;
    }
}

#[tokio::test]
async fn fake_worker_does_not_ignore_registration_or_result_write_failures() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    assert!(
        worker::run(
            &fixture.journal,
            id(4),
            "wrong instance",
            FakeAgent::new(id(1), id(2))
        )
        .await
        .is_err()
    );
    trigger(
        &fixture,
        "pending_events",
        "BEFORE INSERT",
        "1",
        "SELECT RAISE(ABORT,'result write failed');",
    )
    .await;
    assert!(
        worker::run(
            &fixture.journal,
            id(4),
            &instance().to_string(),
            FakeAgent::new(id(1), id(2))
        )
        .await
        .is_err()
    );
    assert_eq!(fixture.status(6).await, CommandStatus::Executing);
    fixture.journal.close().await;
}

#[tokio::test]
async fn concurrent_submitters_get_only_one_launch_and_one_worker_claim() {
    let fixture = Fixture::new().await;
    let other = Journal::attach(fixture.journal.path())
        .await
        .expect("other connection");
    let command = start();
    let (first, second) = tokio::join!(
        fixture.journal.accept(&command, instance()),
        other.accept(&command, Uuid::new_v4())
    );
    let (first, second) = (first.expect("first"), second.expect("second"));
    assert_ne!(first.launch.is_some(), second.launch.is_some());
    let token = fixture.session().await.instance_id;
    let (first, second) = tokio::join!(
        fixture.journal.register(id(4), &token, 42),
        other.register(id(4), &token, 43)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let (first, second) = tokio::join!(
        fixture.journal.claim(id(4), &token),
        other.claim(id(4), &token)
    );
    assert_ne!(
        first.expect("first claim").is_some(),
        second.expect("second claim").is_some()
    );
    other.close().await;
    fixture.journal.close().await;
}

use uuid::Uuid;
