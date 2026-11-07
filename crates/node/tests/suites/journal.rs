//! Real `SQLite` tests: persistence, ordering, fencing, rollback, and outbox delivery.
#![allow(clippy::expect_used)]

use crate::common;

use common::{Fixture, cancel, id, identity, instance, outcome, send, start};
use orchestrator_node::{
    NodeError,
    journal::Journal,
    model::{CommandOutcome, CommandStatus, SessionStatus},
};
use orchestrator_protocol::{agent::AgentEventKind, values::Epoch};
use uuid::Uuid;

#[tokio::test]
async fn schema_is_durable_and_bound_to_one_identity() {
    let fixture = Fixture::new().await;
    let raw = fixture.raw().await;
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(&raw)
            .await
            .expect("mode"),
        "wal"
    );
    fixture.completed().await;
    fixture.journal.close().await;
    let reopened = Journal::open(fixture.journal.path(), identity())
        .await
        .expect("reopen");
    assert_eq!(
        reopened
            .command(id(6))
            .await
            .expect("receipt")
            .expect("saved")
            .status,
        CommandStatus::Completed
    );
    assert_eq!(reopened.pending_events().await.expect("events").len(), 1);
    let attached = Journal::attach(fixture.journal.path())
        .await
        .expect("worker attach");
    assert_eq!(attached.identity(), identity());
    let mut other = identity();
    other.node_id = id(999);
    assert!(matches!(
        Journal::open(fixture.journal.path(), other).await,
        Err(NodeError::Rejected(_))
    ));
    attached.close().await;
    reopened.close().await;
    raw.close().await;
}

#[tokio::test]
async fn missing_corrupt_and_modified_databases_fail_closed() {
    let temp = tempfile::tempdir().expect("directory");
    assert!(
        Journal::attach(&temp.path().join("missing.sqlite"))
            .await
            .is_err()
    );
    assert!(
        Journal::open(&temp.path().join("missing/child.sqlite"), identity())
            .await
            .is_err()
    );
    let fixture = Fixture::new().await;
    let raw = fixture.raw().await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00'")
        .execute(&raw)
        .await
        .expect("tamper checksum");
    assert!(matches!(
        Journal::open(fixture.journal.path(), identity()).await,
        Err(NodeError::Migration(_))
    ));
    sqlx::query("DELETE FROM node_identity")
        .execute(&raw)
        .await
        .expect("remove identity");
    assert!(Journal::attach(fixture.journal.path()).await.is_err());
    raw.close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn mounts_validate_directories_and_cannot_retarget_live_work() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .journal
            .mount(id(5), &fixture.temp.path().join("absent"))
            .await
            .is_err()
    );
    assert!(
        fixture
            .journal
            .mount(id(5), fixture.journal.path())
            .await
            .is_err()
    );
    let second = tempfile::tempdir().expect("second workspace");
    fixture.accepted().await;
    fixture
        .journal
        .mount(id(5), fixture.temp.path())
        .await
        .expect("identical registration");
    assert!(fixture.journal.mount(id(5), second.path()).await.is_err());
    fixture
        .journal
        .stopped(id(4), &instance().to_string())
        .await
        .expect("stop");
    fixture
        .journal
        .mount(id(5), second.path())
        .await
        .expect("now safe to change");
    fixture.journal.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_mount_is_rejected_without_lossy_conversion() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new().await;
    let path = fixture
        .temp
        .path()
        .join(std::ffi::OsString::from_vec(vec![0xff]));
    assert!(matches!(
        fixture.journal.mount(id(5), &path).await,
        Err(NodeError::Rejected(_))
    ));
    fixture.journal.close().await;
}

#[tokio::test]
async fn acceptance_deduplicates_and_fences_the_target() {
    let fixture = Fixture::new().await;
    for other_group in [true, false] {
        let mut wrong = start();
        if other_group {
            wrong.group_id = id(100);
        } else {
            wrong.node_id = id(100);
        }
        assert!(fixture.journal.accept(&wrong, instance()).await.is_err());
    }
    assert!(fixture.journal.accept(&send(11), instance()).await.is_err());
    assert!(fixture.journal.accept(&start(), Uuid::nil()).await.is_err());
    let accepted = fixture
        .journal
        .accept(&start(), instance())
        .await
        .expect("start");
    assert!(accepted.launch.is_some());
    assert_eq!(accepted.record.status, CommandStatus::Queued);
    assert!(
        fixture
            .journal
            .accept(&start(), Uuid::new_v4())
            .await
            .expect("replay")
            .launch
            .is_none()
    );
    assert!(fixture.journal.accept(&send(6), instance()).await.is_err());
    let mut duplicate = start();
    duplicate.command_id = id(10);
    assert!(
        fixture
            .journal
            .accept(&duplicate, instance())
            .await
            .is_err()
    );
    let mut wrong = send(11);
    wrong.target.task_id = id(99);
    assert!(fixture.journal.accept(&wrong, instance()).await.is_err());
    wrong = send(11);
    wrong.target.execution_epoch = Epoch::INITIAL.next().expect("epoch");
    assert!(fixture.journal.accept(&wrong, instance()).await.is_err());
    fixture
        .journal
        .accept(&send(11), instance())
        .await
        .expect("queue follow-up");
    fixture
        .journal
        .interrupted(id(4), &instance().to_string(), "lost")
        .await
        .expect("interrupt");
    assert!(fixture.journal.accept(&send(12), instance()).await.is_err());
    fixture
        .journal
        .heartbeat(id(4), &instance().to_string())
        .await
        .expect("same worker returned");
    fixture
        .journal
        .request_stop_all()
        .await
        .expect("request stop");
    assert!(fixture.journal.accept(&send(12), instance()).await.is_err());
    fixture.journal.close().await;
}

#[tokio::test]
async fn missing_mount_and_failed_insertion_leave_no_launch_intent() {
    let fixture = Fixture::new().await;
    let raw = fixture.raw().await;
    sqlx::query("DELETE FROM workspace_mounts")
        .execute(&raw)
        .await
        .expect("remove mount");
    assert!(fixture.journal.accept(&start(), instance()).await.is_err());
    assert!(
        fixture
            .journal
            .session(id(4))
            .await
            .expect("session")
            .is_none()
    );
    fixture
        .journal
        .mount(id(5), fixture.temp.path())
        .await
        .expect("restore mount");
    sqlx::raw_sql("CREATE TRIGGER fail_insert BEFORE INSERT ON command_receipts BEGIN SELECT RAISE(ABORT,'disk insertion failed'); END;").execute(&raw).await.expect("inject failure");
    assert!(fixture.journal.accept(&start(), instance()).await.is_err());
    assert!(
        fixture
            .journal
            .session(id(4))
            .await
            .expect("rolled back session")
            .is_none()
    );
    assert!(
        fixture
            .journal
            .command(id(6))
            .await
            .expect("rolled back command")
            .is_none()
    );
    sqlx::query("DROP TRIGGER fail_insert")
        .execute(&raw)
        .await
        .expect("repair");
    assert!(
        fixture
            .journal
            .accept(&start(), instance())
            .await
            .expect("retry acceptance")
            .launch
            .is_some()
    );
    raw.close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn worker_registration_is_once_only_and_expiry_never_reexecutes() {
    let fixture = Fixture::new().await;
    fixture.accepted().await;
    assert!(fixture.journal.register(id(4), "wrong", 42).await.is_err());
    assert!(
        fixture
            .journal
            .register(id(4), &instance().to_string(), 0)
            .await
            .is_err()
    );
    fixture
        .journal
        .register(id(4), &instance().to_string(), 42)
        .await
        .expect("register once");
    assert!(
        fixture
            .journal
            .register(id(4), &instance().to_string(), 43)
            .await
            .is_err()
    );
    assert!(fixture.journal.heartbeat(id(4), "wrong").await.is_err());
    assert!(
        fixture
            .journal
            .claim(id(4), "wrong")
            .await
            .expect("wrong instance")
            .is_none()
    );
    fixture
        .journal
        .heartbeat(id(4), &instance().to_string())
        .await
        .expect("alive");
    assert!(fixture.journal.expire(-1).await.is_err());
    fixture.journal.expire(0).await.expect("expire immediately");
    assert_eq!(fixture.session().await.state, SessionStatus::Interrupted);
    assert_eq!(fixture.status(6).await, CommandStatus::Uncertain);
    fixture
        .journal
        .heartbeat(id(4), &instance().to_string())
        .await
        .expect("original returned");
    assert!(
        fixture
            .journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("claim")
            .is_none()
    );
    fixture
        .journal
        .stopped(id(4), "wrong")
        .await
        .expect("stale stop is harmless");
    assert_eq!(fixture.session().await.state, SessionStatus::Running);
    fixture
        .journal
        .stopped(id(4), &instance().to_string())
        .await
        .expect("stop");
    fixture
        .journal
        .stopped(id(4), &instance().to_string())
        .await
        .expect("repeat stop");
    assert!(
        fixture
            .journal
            .heartbeat(id(4), &instance().to_string())
            .await
            .is_err()
    );
    fixture.journal.close().await;
}

#[tokio::test]
async fn fifo_claims_and_atomic_outbox_survive_reopening() {
    let fixture = Fixture::new().await;
    fixture.registered().await;
    fixture
        .journal
        .accept(&send(11), instance())
        .await
        .expect("follow-up");
    let first = fixture
        .journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim")
        .expect("work");
    assert_eq!(first.request.command_id, id(6));
    assert!(
        fixture
            .journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("serial claim")
            .is_none()
    );
    let raw = fixture.raw().await;
    sqlx::raw_sql("CREATE TRIGGER fail_event BEFORE INSERT ON pending_events BEGIN SELECT RAISE(ABORT,'event failure'); END;").execute(&raw).await.expect("inject outbox failure");
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
            .expect("no partial events")
            .is_empty()
    );
    sqlx::query("DROP TRIGGER fail_event")
        .execute(&raw)
        .await
        .expect("repair");
    fixture
        .journal
        .complete(id(6), &instance().to_string(), &outcome(&start()))
        .await
        .expect("complete");
    fixture
        .journal
        .complete(id(6), &instance().to_string(), &outcome(&start()))
        .await
        .expect("repeat completion");
    let second = fixture
        .journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim next")
        .expect("work");
    assert_eq!(second.request.command_id, id(11));
    let rejected = CommandOutcome::Rejected {
        reason: "intentional rejection".to_owned(),
    };
    fixture
        .journal
        .complete(id(11), &instance().to_string(), &rejected)
        .await
        .expect("reject");
    assert_eq!(fixture.status(11).await, CommandStatus::Rejected);
    let events = fixture.journal.pending_events().await.expect("outbox");
    assert_eq!(events.len(), 2);
    assert!(events[0].sequence < events[1].sequence);
    assert!(
        fixture
            .journal
            .acknowledge(events[0].sequence)
            .await
            .expect("ack")
    );
    assert!(
        fixture
            .journal
            .acknowledge(events[0].sequence)
            .await
            .expect("repeat ack")
    );
    assert!(!fixture.journal.acknowledge(-1).await.expect("unknown ack"));
    assert_eq!(
        fixture
            .journal
            .pending_events()
            .await
            .expect("remaining")
            .len(),
        1
    );
    raw.close().await;
    fixture.journal.close().await;
}

#[tokio::test]
async fn results_must_match_claimed_command_and_worker() {
    let fixture = Fixture::new().await;
    fixture.registered().await;
    assert!(
        fixture
            .journal
            .complete(id(999), &instance().to_string(), &outcome(&start()))
            .await
            .is_err()
    );
    assert!(
        fixture
            .journal
            .complete(id(6), &instance().to_string(), &outcome(&start()))
            .await
            .is_err()
    );
    fixture
        .journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    assert!(
        fixture
            .journal
            .complete(id(6), "wrong", &outcome(&start()))
            .await
            .is_err()
    );
    for change in 0..7 {
        let CommandOutcome::Completed { mut event } = outcome(&start()) else {
            unreachable!()
        };
        match change {
            0 => event.command_id = id(99),
            1 => event.group_id = id(99),
            2 => event.node_id = id(99),
            3 => event.target.session_id = id(99),
            4 => event.target.execution_epoch = Epoch::INITIAL.next().expect("epoch"),
            5 => event.kind = AgentEventKind::Cancelled,
            _ => {
                if let AgentEventKind::Replied { message_id, .. } = &mut event.kind {
                    *message_id = id(99);
                }
            }
        }
        assert!(
            fixture
                .journal
                .complete(
                    id(6),
                    &instance().to_string(),
                    &CommandOutcome::Completed { event }
                )
                .await
                .is_err()
        );
    }
    fixture
        .journal
        .complete(id(6), &instance().to_string(), &outcome(&start()))
        .await
        .expect("valid");
    assert!(
        fixture
            .journal
            .complete(
                id(6),
                &instance().to_string(),
                &CommandOutcome::Rejected {
                    reason: "contradiction".to_owned()
                }
            )
            .await
            .is_err()
    );
    fixture
        .journal
        .accept(&cancel(12), instance())
        .await
        .expect("cancel");
    fixture
        .journal
        .accept(&send(13), instance())
        .await
        .expect("queued behind cancel");
    fixture
        .journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim cancel");
    fixture
        .journal
        .complete(id(12), &instance().to_string(), &outcome(&cancel(12)))
        .await
        .expect("cancel result");
    assert_eq!(fixture.session().await.state, SessionStatus::Stopped);
    assert_eq!(fixture.status(13).await, CommandStatus::Uncertain);
    fixture.journal.close().await;
}
