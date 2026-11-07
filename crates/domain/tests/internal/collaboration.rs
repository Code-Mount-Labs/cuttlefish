#![allow(clippy::expect_used)]
use super::*;
use orchestrator_protocol::collaboration::Progress;
#[path = "../../../protocol/tests/fixtures/mod.rs"]
mod data;
use data::{change, goal, id, release, task};
fn text(s: &str) -> orchestrator_protocol::values::MessageText {
    s.to_owned().try_into().expect("text")
}
fn base() -> GoalView {
    create(goal(), id(2))
}
fn act(view: &GoalView, action: GoalAction) -> GoalView {
    apply(view, id(2), &change(view, action), &[release(300)]).expect("valid action")
}
fn planned() -> GoalView {
    act(
        &base(),
        GoalAction::Plan {
            tasks: vec![task(200)],
        },
    )
}
fn execution(spec: &TaskSpec) -> DispatchView {
    let mut event = data::report().events.remove(0);
    event.command.command_id = spec.request.command_id;
    if let RouteAction::Start {
        task_id,
        session_id,
        ..
    } = spec.request.action
    {
        event.command.target.task_id = task_id;
        event.command.target.session_id = session_id;
    }
    DispatchView {
        request: spec.request.clone(),
        command: event.command,
        state: orchestrator_protocol::routing::DeliveryState::Completed,
        outcome: Some(event.outcome),
    }
}
#[test]
fn goal_plans_dependencies_progress_discussion_and_verifies_results() {
    let mut second = task(210);
    second.dependencies.insert(id(202));
    let mut view = act(
        &base(),
        GoalAction::Plan {
            tasks: vec![task(200), second],
        },
    );
    assert!(
        apply(
            &view,
            id(2),
            &change(&view, GoalAction::Delegate { task_id: id(212) }),
            &[]
        )
        .is_err()
    );
    view = act(&view, GoalAction::Pause {});
    assert!(
        apply(
            &view,
            id(2),
            &change(&view, GoalAction::Delegate { task_id: id(202) }),
            &[]
        )
        .is_err()
    );
    view = act(&view, GoalAction::Resume {});
    view = act(&view, GoalAction::Delegate { task_id: id(202) });
    let observed = execution(&view.tasks[0].spec);
    observe(&mut view, observed).expect("observation");
    view = act(
        &view,
        GoalAction::Progress {
            task_id: id(202),
            progress: Progress {
                completed: 1,
                total: 2,
                note: text("one artifact"),
            },
        },
    );
    view = act(
        &view,
        GoalAction::Discuss {
            recipient: Some(id(2)),
            text: text("reviewed the evidence"),
        },
    );
    view = act(
        &view,
        GoalAction::Verify {
            task_id: id(202),
            evidence: text("checked output"),
        },
    );
    view = act(&view, GoalAction::Delegate { task_id: id(212) });
    let observed = execution(&view.tasks[1].spec);
    observe(&mut view, observed).expect("observation");
    view = act(
        &view,
        GoalAction::Verify {
            task_id: id(212),
            evidence: text("tested application"),
        },
    );
    view = act(
        &view,
        GoalAction::Finish {
            result: text("both artifacts completed"),
        },
    );
    assert_eq!(view.status, GoalStatus::Completed);
    assert!(apply(&view, id(2), &change(&view, GoalAction::Resume {}), &[]).is_err());
}
#[test]
fn learning_is_native_approved_evaluated_and_replicated_per_node() {
    assert!(
        orchestrator_protocol::capabilities::Capability::native()
            .contains(&"agent.learn".to_owned().try_into().expect("cap"))
    );
    let mut view = planned();
    let learn = GoalAction::Learn {
        task_id: id(202),
        node_id: id(2),
        release_id: id(300),
        rationale: text("missing tool"),
    };
    assert!(apply(&view, id(2), &change(&view, learn.clone()), &[]).is_err());
    view = act(&view, learn);
    assert!(participant(&view, id(2)));
    assert!(!participant(&view, id(99)));
    assert!(
        decide(
            &view,
            view.revision,
            id(300),
            id(2),
            &LearningDecision::Activate {}
        )
        .is_err()
    );
    view = decide(
        &view,
        view.revision,
        id(300),
        id(2),
        &LearningDecision::Approve {},
    )
    .expect("approval");
    view = act(
        &view,
        GoalAction::Evaluate {
            release_id: id(300),
            passed: false,
            evidence: text("regression found"),
        },
    );
    assert!(
        decide(
            &view,
            view.revision,
            id(300),
            id(2),
            &LearningDecision::Activate {}
        )
        .is_err()
    );
    view = act(
        &view,
        GoalAction::Evaluate {
            release_id: id(300),
            passed: true,
            evidence: text("setup and conformance checks pass"),
        },
    );
    view = decide(
        &view,
        view.revision,
        id(300),
        id(2),
        &LearningDecision::Activate {},
    )
    .expect("activation");
    replicate_and_reject(view);
}

fn replicate_and_reject(mut view: GoalView) {
    view = act(
        &view,
        GoalAction::Replicate {
            release_id: id(300),
            nodes: [id(9)].into(),
            rationale: text("user requested replication"),
        },
    );
    assert!(participant(&view, id(9)));
    assert!(matches!(
        view.learning[1].status,
        LearningStatus::Proposed {}
    ));
    assert!(
        apply(
            &view,
            id(9),
            &change(
                &view,
                GoalAction::Evaluate {
                    release_id: id(300),
                    passed: true,
                    evidence: text("not approved")
                }
            ),
            &[]
        )
        .is_err()
    );
    view = decide(
        &view,
        view.revision,
        id(300),
        id(9),
        &LearningDecision::Reject {
            reason: text("not needed"),
        },
    )
    .expect("reject");
    assert!(matches!(
        view.learning[1].status,
        LearningStatus::Rejected { .. }
    ));
}
#[test]
fn invalid_plans_are_atomic_and_cannot_inject_unknown_dependencies() {
    for tasks in [vec![], vec![task(200); 65], vec![task(200); 2]] {
        let view = base();
        assert!(
            apply(
                &view,
                id(2),
                &change(&view, GoalAction::Plan { tasks }),
                &[]
            )
            .is_err()
        );
        assert!(view.tasks.is_empty());
    }
    for kind in 0..6 {
        let first = task(200);
        let mut second = task(210);
        match kind {
            0 => second.request.thread_id = first.request.thread_id,
            1 => second.request.command_id = first.request.command_id,
            2 => {
                if let RouteAction::Start { session_id, .. } = &mut second.request.action {
                    *session_id = id(203);
                }
            }
            3 => {
                second.dependencies.insert(id(212));
            }
            4 => {
                second.dependencies.insert(id(999));
            }
            _ => second.request.thread_id = goal().thread_id,
        }
        assert!(
            apply(
                &base(),
                id(2),
                &change(
                    &base(),
                    GoalAction::Plan {
                        tasks: vec![first, second]
                    }
                ),
                &[]
            )
            .is_err()
        );
    }
    let mut invalid = task(200);
    invalid.request.action = RouteAction::Cancel {};
    assert!(task_id(&invalid).is_err());
    assert!(
        apply(
            &base(),
            id(2),
            &change(
                &base(),
                GoalAction::Plan {
                    tasks: vec![invalid]
                }
            ),
            &[]
        )
        .is_err()
    );
    let mut invalid = task(200);
    if let RouteAction::Start { capabilities, .. } = &mut invalid.request.action {
        *capabilities = (0..65)
            .map(|i| format!("cap{i}").try_into().expect("cap"))
            .collect();
    }
    assert!(
        apply(
            &base(),
            id(2),
            &change(
                &base(),
                GoalAction::Plan {
                    tasks: vec![invalid]
                }
            ),
            &[]
        )
        .is_err()
    );
    assert!(validate_plan(&base()).is_ok());
}
#[test]
fn rejects_stale_unauthorized_premature_and_regressing_updates() {
    let view = planned();
    let progress = GoalAction::Progress {
        task_id: id(202),
        progress: Progress {
            completed: 0,
            total: 1,
            note: text("start"),
        },
    };
    for action in [
        GoalAction::Finish {
            result: text("premature"),
        },
        GoalAction::Verify {
            task_id: id(202),
            evidence: text("no execution"),
        },
        GoalAction::Delegate { task_id: id(999) },
        progress.clone(),
        GoalAction::Evaluate {
            release_id: id(999),
            passed: true,
            evidence: text("unknown"),
        },
    ] {
        assert!(apply(&view, id(2), &change(&view, action), &[]).is_err());
    }
    assert!(apply(&view, id(99), &change(&view, GoalAction::Pause {}), &[]).is_err());
    assert!(
        apply(
            &view,
            id(99),
            &change(
                &view,
                GoalAction::Discuss {
                    recipient: None,
                    text: text("unauthorized")
                }
            ),
            &[]
        )
        .is_err()
    );
    assert!(
        apply(
            &view,
            id(2),
            &change(
                &view,
                GoalAction::Discuss {
                    recipient: Some(id(99)),
                    text: text("unknown recipient")
                }
            ),
            &[]
        )
        .is_err()
    );
    let mut stale = change(&view, GoalAction::Pause {});
    stale.expected_revision = 99;
    assert!(apply(&view, id(2), &stale, &[]).is_err());
    assert!(decide(&view, 99, id(300), id(2), &LearningDecision::Approve {}).is_err());
    check_progress(view, progress);
}

fn check_progress(mut view: GoalView, progress: GoalAction) {
    view = act(&view, GoalAction::Delegate { task_id: id(202) });
    let observed = execution(&view.tasks[0].spec);
    observe(&mut view, observed).expect("result");
    view = act(
        &view,
        GoalAction::Progress {
            task_id: id(202),
            progress: Progress {
                completed: 1,
                total: 2,
                note: text("progress"),
            },
        },
    );
    for (completed, total) in [(0, 0), (3, 2), (0, 2), (1, 3)] {
        assert!(
            apply(
                &view,
                id(2),
                &change(
                    &view,
                    GoalAction::Progress {
                        task_id: id(202),
                        progress: Progress {
                            completed,
                            total,
                            note: text("bad")
                        }
                    }
                ),
                &[]
            )
            .is_err()
        );
    }
    assert!(apply(&view, id(9), &change(&view, progress), &[]).is_err());
    assert!(
        apply(
            &view,
            id(2),
            &change(&view, GoalAction::Delegate { task_id: id(202) }),
            &[]
        )
        .is_err()
    );
}

#[test]
fn boundaries_reject_invalid_observations_and_learning_transitions() {
    let mut view = planned();
    let observed = execution(&view.tasks[0].spec);
    assert!(observe(&mut view, observed.clone()).is_err());
    assert!(observe(&mut base(), observed.clone()).is_err());
    view = act(&view, GoalAction::Delegate { task_id: id(202) });
    let mut bad = observed.clone();
    bad.request.thread_id = id(900);
    assert!(observe(&mut view, bad).is_err());
    let mut bad = observed.clone();
    bad.command.action = AgentAction::Cancel;
    assert!(observe(&mut view, bad).is_err());
    observe(&mut view, observed).expect("observation");
    let learn = GoalAction::Learn {
        task_id: id(202),
        node_id: id(2),
        release_id: id(300),
        rationale: text("learn"),
    };
    assert!(apply(&view, id(2), &change(&view, learn.clone()), &[release(300)]).is_err());
    let mut view = planned();
    let mut unrelated = release(300);
    unrelated.capability = "draw.image".to_owned().try_into().expect("capability");
    assert!(apply(&view, id(2), &change(&view, learn.clone()), &[unrelated]).is_err());
    view = act(&view, learn.clone());
    assert!(apply(&view, id(2), &change(&view, learn), &[release(300)]).is_err());
    assert!(
        apply(
            &view,
            id(2),
            &change(
                &view,
                GoalAction::Replicate {
                    release_id: id(300),
                    nodes: [id(9)].into(),
                    rationale: text("replicate")
                }
            ),
            &[]
        )
        .is_err()
    );
    view.learning[0].status = LearningStatus::Active {};
    for nodes in [BTreeSet::new(), [id(2)].into()] {
        assert!(
            apply(
                &view,
                id(2),
                &change(
                    &view,
                    GoalAction::Replicate {
                        release_id: id(300),
                        nodes,
                        rationale: text("replicate")
                    }
                ),
                &[]
            )
            .is_err()
        );
    }
    check_revision_and_discussion_limits(view);
}

fn check_revision_and_discussion_limits(mut view: GoalView) {
    view.revision = u64::MAX;
    assert!(apply(&view, id(2), &change(&view, GoalAction::Pause {}), &[]).is_err());
    assert!(
        decide(
            &view,
            u64::MAX,
            id(300),
            id(2),
            &LearningDecision::Reject {
                reason: text("reject")
            }
        )
        .is_err()
    );
    let mut view = planned();
    view.discussion = vec![
        Discussion {
            author: id(2),
            recipient: None,
            text: text("message")
        };
        128
    ];
    assert!(
        apply(
            &view,
            id(2),
            &change(
                &view,
                GoalAction::Discuss {
                    recipient: None,
                    text: text("overflow")
                }
            ),
            &[]
        )
        .is_err()
    );
    let evaluate = GoalAction::Evaluate {
        release_id: id(300),
        passed: true,
        evidence: text("test"),
    };
    assert!(apply_owner(&mut view, &evaluate, &[]).is_err());
    assert!(apply_learning(&mut view, &GoalAction::Pause {}, &[]).is_err());
    assert!(
        decide(
            &view,
            view.revision,
            id(999),
            id(2),
            &LearningDecision::Approve {}
        )
        .is_err()
    );
}

#[test]
fn unknown_task_operations_and_non_reply_results_are_rejected() {
    let view = planned();
    for action in [
        GoalAction::Verify {
            task_id: id(999),
            evidence: text("none"),
        },
        GoalAction::Learn {
            task_id: id(999),
            node_id: id(2),
            release_id: id(300),
            rationale: text("none"),
        },
        GoalAction::Progress {
            task_id: id(999),
            progress: Progress {
                completed: 1,
                total: 1,
                note: text("none"),
            },
        },
    ] {
        assert!(apply(&view, id(2), &change(&view, action), &[release(300)]).is_err());
    }
    let mut result = execution(&view.tasks[0].spec);
    if let Some(CommandOutcome::Completed { event }) = &mut result.outcome {
        event.kind = orchestrator_protocol::agent::AgentEventKind::Cancelled;
    }
    assert!(!successful(&result));
}
