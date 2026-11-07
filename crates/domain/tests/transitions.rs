//! Exhaustive lifecycle tests, including cancellation races and restart safety.
#![allow(clippy::expect_used)]

use std::error::Error;

use orchestrator_domain::{
    ExecutionObservation, PreviousExecution, TaskAction, TaskState, TransitionError,
};
use orchestrator_protocol::{task::TaskStatus, values::Epoch};

const RESTART: TaskAction = TaskAction::Restart {
    previous_execution: PreviousExecution::Stopped,
};
const UNSAFE_RESTART: TaskAction = TaskAction::Restart {
    previous_execution: PreviousExecution::Unknown,
};

fn recovery_cases(epoch: Epoch) -> [(TaskAction, TaskStatus); 5] {
    use TaskAction as A;
    use TaskStatus as S;
    [
        (ExecutionObservation::Running, S::Running),
        (ExecutionObservation::WaitingInput, S::WaitingInput),
        (ExecutionObservation::Completed, S::Completed),
        (ExecutionObservation::Failed, S::Failed),
        (ExecutionObservation::Cancelled, S::Cancelled),
    ]
    .map(|(observed, status)| {
        (
            A::Reconcile {
                execution_epoch: epoch,
                observed,
            },
            status,
        )
    })
}

#[test]
fn every_status_action_pair_has_an_explicit_outcome() {
    use TaskAction as A;
    use TaskStatus as S;
    let epoch = Epoch::try_from(7).expect("valid epoch");
    let recoveries = recovery_cases(epoch);
    let mut actions = vec![
        A::Begin,
        A::RequestInput,
        A::Resume,
        A::Complete,
        A::Fail,
        A::Interrupt,
        A::RequestCancel,
        A::ConfirmCancel,
        RESTART,
        UNSAFE_RESTART,
    ];
    actions.extend(recoveries.iter().map(|(action, _)| *action));
    let mut interrupted = vec![(A::RequestCancel, S::Cancelling), (RESTART, S::Queued)];
    interrupted.extend(recoveries);
    let cases: [(TaskStatus, &[(TaskAction, TaskStatus)]); 8] = [
        (
            S::Queued,
            &[
                (A::Begin, S::Running),
                (A::Fail, S::Failed),
                (A::RequestCancel, S::Cancelled),
            ],
        ),
        (
            S::Running,
            &[
                (A::RequestInput, S::WaitingInput),
                (A::Complete, S::Completed),
                (A::Fail, S::Failed),
                (A::Interrupt, S::Interrupted),
                (A::RequestCancel, S::Cancelling),
            ],
        ),
        (
            S::WaitingInput,
            &[
                (A::Resume, S::Running),
                (A::Fail, S::Failed),
                (A::Interrupt, S::Interrupted),
                (A::RequestCancel, S::Cancelling),
            ],
        ),
        (
            S::Cancelling,
            &[
                (A::Complete, S::Completed),
                (A::Fail, S::Failed),
                (A::Interrupt, S::Interrupted),
                (A::ConfirmCancel, S::Cancelled),
            ],
        ),
        (S::Completed, &[]),
        (S::Failed, &[(RESTART, S::Queued)]),
        (S::Interrupted, &interrupted),
        (S::Cancelled, &[(RESTART, S::Queued)]),
    ];
    for (status, allowed) in cases {
        let original = TaskState::restore(status, epoch);
        for &action in &actions {
            let result = original.transition(action);
            if let Some((_, expected)) = allowed.iter().find(|(candidate, _)| *candidate == action)
            {
                let next = result.expect("allowed transition");
                assert_eq!(next.status(), *expected, "{status:?} + {action:?}");
                assert_eq!(
                    next.execution_epoch().get(),
                    if action == RESTART { 8 } else { 7 }
                );
            } else if action == UNSAFE_RESTART
                && allowed.iter().any(|(candidate, _)| *candidate == RESTART)
            {
                assert_eq!(result, Err(TransitionError::UnresolvedExecution));
            } else {
                assert_eq!(result, Err(TransitionError::Invalid { status, action }));
            }
            assert_eq!(
                original,
                TaskState::restore(status, epoch),
                "source is immutable"
            );
        }
    }
}

#[test]
fn recovery_rejects_stale_and_future_execution_reports() {
    let epoch = Epoch::try_from(7).expect("epoch");
    let state = TaskState::restore(TaskStatus::Interrupted, epoch);
    for received in [Epoch::INITIAL, epoch.next().expect("next")] {
        assert_eq!(
            state.transition(TaskAction::Reconcile {
                execution_epoch: received,
                observed: ExecutionObservation::Completed,
            }),
            Err(TransitionError::EpochMismatch {
                expected: epoch,
                received
            })
        );
    }
    assert_eq!(state.status(), TaskStatus::Interrupted);
}

#[test]
fn initial_state_and_followups_keep_the_execution_epoch() {
    let queued = TaskState::new();
    assert_eq!(queued, TaskState::default());
    assert_eq!(queued.status(), TaskStatus::Queued);
    let mut state = queued.transition(TaskAction::Begin).expect("begin");
    for _ in 0..100 {
        state = state
            .transition(TaskAction::RequestInput)
            .expect("wait")
            .transition(TaskAction::Resume)
            .expect("resume");
        assert_eq!(state.execution_epoch(), Epoch::INITIAL);
    }
    assert_eq!(
        state
            .transition(TaskAction::Complete)
            .expect("complete")
            .status(),
        TaskStatus::Completed
    );
}

#[test]
fn restarting_advances_epochs_and_cannot_wrap() {
    let mut state = TaskState::new();
    for expected in 2..102 {
        state = state
            .transition(TaskAction::RequestCancel)
            .expect("queued cancellation")
            .transition(RESTART)
            .expect("explicit restart");
        assert_eq!(state.execution_epoch().get(), expected);
    }
    let maximum = TaskState::restore(
        TaskStatus::Interrupted,
        Epoch::try_from(u64::MAX).expect("maximum"),
    );
    assert_eq!(
        maximum.transition(RESTART),
        Err(TransitionError::EpochExhausted)
    );
    assert_eq!(maximum.execution_epoch().get(), u64::MAX);
}

#[test]
fn loss_of_contact_is_not_confirmation_of_cancellation() {
    let interrupted = TaskState::new()
        .transition(TaskAction::Begin)
        .expect("begin")
        .transition(TaskAction::Interrupt)
        .expect("disconnect");
    assert!(interrupted.transition(TaskAction::ConfirmCancel).is_err());
    assert_eq!(
        interrupted.transition(UNSAFE_RESTART),
        Err(TransitionError::UnresolvedExecution)
    );
    let stopping = interrupted
        .transition(TaskAction::RequestCancel)
        .expect("request stop");
    assert_eq!(stopping.status(), TaskStatus::Cancelling);
    assert_eq!(
        stopping
            .transition(TaskAction::ConfirmCancel)
            .expect("confirmed stop")
            .status(),
        TaskStatus::Cancelled
    );
}

#[test]
fn errors_preserve_context_and_do_not_hide_failure() {
    let invalid = TransitionError::Invalid {
        status: TaskStatus::Queued,
        action: TaskAction::Complete,
    };
    assert_eq!(invalid.to_string(), "cannot apply Complete to Queued");
    assert_eq!(
        TransitionError::UnresolvedExecution.to_string(),
        "restart requires a resolved previous execution"
    );
    assert_eq!(
        TransitionError::EpochExhausted.to_string(),
        "execution epoch exhausted"
    );
    let mismatch = TransitionError::EpochMismatch {
        expected: Epoch::INITIAL,
        received: Epoch::INITIAL.next().expect("next"),
    };
    assert_eq!(
        mismatch.to_string(),
        "execution epoch mismatch: expected 1, received 2"
    );
    for error in [
        invalid,
        TransitionError::UnresolvedExecution,
        TransitionError::EpochExhausted,
        mismatch,
    ] {
        assert!(error.source().is_none());
    }
}
