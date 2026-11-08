#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, instance};
struct Uncertain;
impl Agent for Uncertain {
    type Error = std::io::Error;
    fn failure_is_uncertain(&self) -> bool {
        true
    }
    async fn execute(
        &mut self,
        _: &AgentCommand,
    ) -> Result<orchestrator_protocol::agent::AgentEvent, Self::Error> {
        Err(std::io::Error::other("unknown effects"))
    }
}
#[tokio::test]
async fn external_provider_failure_is_durable_uncertainty_not_a_replayable_rejection() {
    for fail_save in [false, true] {
        let f = Fixture::new().await;
        f.accepted().await;
        if fail_save {
            let pool = f.raw().await;
            sqlx::query("CREATE TRIGGER fail_interrupt BEFORE UPDATE ON local_sessions WHEN NEW.state='interrupted' BEGIN SELECT RAISE(ABORT,'write failed'); END").execute(&pool).await.expect("fault");
            pool.close().await;
        }
        assert!(
            run(&f.journal, id(4), &instance().to_string(), Uncertain)
                .await
                .is_err()
        );
        if !fail_save {
            assert_eq!(
                f.journal
                    .command(id(6))
                    .await
                    .expect("command")
                    .expect("saved")
                    .status,
                crate::model::CommandStatus::Uncertain
            );
        }
        if !fail_save {
            f.journal
                .interrupted(id(4), &instance().to_string(), "worker exited")
                .await
                .expect("reap");
            assert!(
                f.journal
                    .session(id(4))
                    .await
                    .expect("session")
                    .expect("saved")
                    .last_error
                    .expect("reason")
                    .contains("unknown effects")
            );
        }
        assert!(f.journal.pending_events().await.expect("outbox").is_empty());
    }
}

#[tokio::test]
async fn known_safe_rejections_preserve_the_session_for_a_later_command() {
    let f = Fixture::new().await;
    f.registered().await;
    let before = f
        .journal
        .session(id(4))
        .await
        .expect("session")
        .expect("exists");
    let outcome = finish_execution(
        &f.journal,
        &instance().to_string(),
        &crate::test_support::start(),
        Err("invalid request before execution".into()),
        false,
    )
    .await
    .expect("safe rejection");
    assert!(
        matches!(outcome, CommandOutcome::Rejected { reason } if reason == "invalid request before execution")
    );
    let after = f
        .journal
        .session(id(4))
        .await
        .expect("session")
        .expect("exists");
    assert_eq!(before.state, after.state);
    assert!(f.journal.pending_events().await.expect("events").is_empty());
}
