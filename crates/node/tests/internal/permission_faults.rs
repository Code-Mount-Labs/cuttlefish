use super::*;

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Bind,
    Request,
    Decide,
    Begin,
    Acknowledge,
    Abandon,
}
const MUTATIONS: [Mutation; 6] = [
    Mutation::Bind,
    Mutation::Request,
    Mutation::Decide,
    Mutation::Begin,
    Mutation::Acknowledge,
    Mutation::Abandon,
];

async fn prepare(operation: Mutation) -> Fixture {
    let f = setup().await;
    match operation {
        Mutation::Bind => sql(&f, "DELETE FROM provider_bindings").await,
        Mutation::Request => (),
        Mutation::Decide | Mutation::Abandon => pending(&f).await,
        Mutation::Begin => approved(&f).await,
        Mutation::Acknowledge => {
            approved(&f).await;
            f.journal
                .begin_permission_delivery(id(200), &instance().to_string())
                .await
                .expect("intent");
        }
    }
    f
}

async fn mutate(journal: &Journal, operation: Mutation) -> Result<(), NodeError> {
    let worker = instance().to_string();
    match operation {
        Mutation::Bind => {
            journal
                .bind_provider(id(6), &worker, &request().binding)
                .await
        }
        Mutation::Request => journal
            .request_permission(&worker, &request())
            .await
            .map(|_| ()),
        Mutation::Decide => journal
            .decide_permission(&token(), &decision(&request()))
            .await
            .map(|_| ()),
        Mutation::Begin => journal
            .begin_permission_delivery(id(200), &worker)
            .await
            .map(|_| ()),
        Mutation::Acknowledge => journal.acknowledge_permission(id(200), &worker).await,
        Mutation::Abandon => journal.abandon_permission(id(200), &worker).await,
    }
}

#[tokio::test]
async fn closed_journal_never_authorizes_any_permission_operation() {
    let f = setup().await;
    f.journal.close().await;
    for operation in MUTATIONS {
        assert!(mutate(&f.journal, operation).await.is_err());
    }
    assert!(f.journal.provider_binding(id(4)).await.is_err());
    assert!(f.journal.permission(id(200)).await.is_err());
    assert!(f.journal.pending_permissions(None).await.is_err());
    assert!(
        f.journal
            .configure_reviewer(id(202), &token(), true)
            .await
            .is_err()
    );
    assert!(deliver(&f.journal, &mut Provider::default()).await.is_err());
}

#[tokio::test]
async fn write_and_commit_failures_preserve_the_previous_permission_state() {
    for operation in MUTATIONS {
        for deferred in [false, true] {
            let f = prepare(operation).await;
            let before = f.journal.permission(id(200)).await.ok();
            let (table, verb) = match operation {
                Mutation::Bind => ("provider_bindings", "INSERT"),
                Mutation::Request => ("runtime_permissions", "INSERT"),
                _ => ("runtime_permissions", "UPDATE"),
            };
            let pool = f.raw().await;
            let body = if deferred {
                sqlx::raw_sql("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED);")
                    .execute(&pool).await.expect("deferred FK");
                "INSERT INTO fault_child VALUES(1);"
            } else {
                "SELECT RAISE(ABORT,'injected write failure');"
            };
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE TRIGGER injected AFTER {verb} ON {table} BEGIN {body} END;"
            )))
            .execute(&pool)
            .await
            .expect("fault trigger");
            assert!(
                mutate(&f.journal, operation).await.is_err(),
                "{operation:?}, deferred={deferred}"
            );
            assert_eq!(
                f.journal.permission(id(200)).await.ok(),
                before,
                "atomic {operation:?}"
            );
            if matches!(operation, Mutation::Bind) {
                assert!(
                    f.journal
                        .provider_binding(id(4))
                        .await
                        .expect("query")
                        .is_none()
                );
            }
            assert_eq!(f.status(6).await, CommandStatus::Executing);
            assert!(f.journal.pending_events().await.expect("outbox").is_empty());
            sqlx::query("DROP TRIGGER injected")
                .execute(&pool)
                .await
                .expect("remove fault");
            mutate(&f.journal, operation)
                .await
                .expect("retry known uncommitted mutation");
            pool.close().await;
            f.journal.close().await;
        }
    }
}

#[tokio::test]
async fn read_failures_do_not_create_permissions_or_discard_decisions() {
    for operation in MUTATIONS {
        let f = prepare(operation).await;
        let statement = if matches!(operation, Mutation::Bind) {
            "DROP TABLE provider_bindings"
        } else {
            "DROP TABLE runtime_permissions"
        };
        sql(&f, statement).await;
        assert!(mutate(&f.journal, operation).await.is_err());
        f.journal.close().await;
    }
    for operation in [Mutation::Decide, Mutation::Begin] {
        let f = prepare(operation).await;
        sql(&f, "DROP TABLE permission_reviewers").await;
        assert!(mutate(&f.journal, operation).await.is_err());
        f.journal.close().await;
    }
    for operation in [Mutation::Request, Mutation::Begin] {
        let f = prepare(operation).await;
        sql(&f, "DROP TABLE provider_bindings").await;
        assert!(mutate(&f.journal, operation).await.is_err());
        f.journal.close().await;
    }
    let f = prepare(Mutation::Bind).await;
    sql(&f, "DROP TABLE command_receipts").await;
    assert!(mutate(&f.journal, Mutation::Bind).await.is_err());
    f.journal.close().await;
}

#[tokio::test]
async fn invalid_persisted_review_and_revoked_ownership_fail_closed() {
    let f = prepare(Mutation::Begin).await;
    let pool = f.raw().await;
    sqlx::raw_sql("PRAGMA ignore_check_constraints=ON; UPDATE runtime_permissions SET record=json_set(record,'$.review',NULL);")
        .execute(&pool).await.expect("corrupt persisted review");
    assert!(mutate(&f.journal, Mutation::Begin).await.is_err());
    pool.close().await;
    f.journal.close().await;
    let f = prepare(Mutation::Abandon).await;
    f.journal.request_stop_all().await.expect("stop requested");
    assert!(mutate(&f.journal, Mutation::Abandon).await.is_err());
    f.journal.close().await;
}

#[tokio::test]
async fn interrupted_clock_read_cannot_make_an_expired_permission_valid() {
    let f = setup().await;
    let mut connection = f.journal.pool.acquire().await.expect("connection");
    connection
        .lock_handle()
        .await
        .expect("handle")
        .set_progress_handler(1, || false);
    assert!(unexpired(&mut connection, &request()).await.is_err());
    connection
        .lock_handle()
        .await
        .expect("handle")
        .remove_progress_handler();
    assert!(unexpired(&mut connection, &request()).await.is_ok());
    drop(connection);
    f.journal.close().await;
}
