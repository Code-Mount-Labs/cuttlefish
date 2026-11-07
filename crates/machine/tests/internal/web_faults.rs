#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{Backend, initialize, registration};
async fn machine() -> (tempfile::TempDir, Machine) {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    (dir, machine)
}
async fn admin(machine: &Machine) -> HeaderMap {
    let session = machine
        .setup_admin(
            &machine.bootstrap_token().expect("token"),
            Credentials {
                username: "operator".into(),
                password: "long-enough-password".into(),
            },
        )
        .await
        .expect("admin");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("other=value; cuttlefish_local_session={session}")
            .parse()
            .expect("cookie"),
    );
    headers.insert(
        header::ORIGIN,
        "http://127.0.0.1:8080".parse().expect("origin"),
    );
    headers
}
async fn sql(machine: &Machine, sql: &'static str) {
    sqlx::raw_sql(sql)
        .execute(&machine.pool)
        .await
        .expect("fault");
}
#[test]
fn malformed_credentials_and_rate_limit_status_are_unambiguous() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_bytes(b"bad=\xff").expect("value"),
    );
    assert!(token(&headers).is_err());
    headers.insert(header::COOKIE, HeaderValue::from_static("other=value"));
    assert!(token(&headers).is_err());
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("cuttlefish_local_session=short"),
    );
    assert!(token(&headers).is_err());
    assert_eq!(
        Error::Limited.into_response().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
#[tokio::test]
async fn handlers_propagate_authentication_and_storage_failures() {
    let (_dir, machine) = machine().await;
    let headers = admin(&machine).await;
    assert!(
        setup(
            State(machine.clone()),
            HeaderMap::new(),
            Json(Setup {
                token: "a".repeat(64),
                username: "operator".into(),
                password: "long-enough-password".into()
            })
        )
        .await
        .is_err()
    );
    assert!(
        setup(
            State(machine.clone()),
            headers.clone(),
            Json(Setup {
                token: "short".into(),
                username: "operator".into(),
                password: "long-enough-password".into()
            })
        )
        .await
        .is_err()
    );
    assert!(
        setup(
            State(machine.clone()),
            headers.clone(),
            Json(Setup {
                token: "a".repeat(64),
                username: "operator".into(),
                password: "long-enough-password".into()
            })
        )
        .await
        .is_err()
    );
    for headers in [HeaderMap::new(), headers.clone()] {
        assert!(
            login(
                State(machine.clone()),
                headers,
                Json(Credentials {
                    username: "operator".into(),
                    password: "wrong-but-long-enough".into()
                })
            )
            .await
            .is_err()
        );
    }
    assert!(
        logout(State(machine.clone()), HeaderMap::new())
            .await
            .is_err()
    );
    assert!(
        retry_database(State(machine.clone()), HeaderMap::new())
            .await
            .is_err()
    );
    assert!(
        retry_database(State(machine.clone()), headers.clone())
            .await
            .is_err()
    );
    let settings = || {
        Json(ModelSettings {
            endpoint: "bad".into(),
            model: "model".into(),
            api_key: "key".into(),
        })
    };
    assert!(
        model(State(machine.clone()), HeaderMap::new(), settings())
            .await
            .is_err()
    );
    assert!(
        model(State(machine.clone()), headers.clone(), settings())
            .await
            .is_err()
    );
    storage_failures(&machine, &headers).await;
}
async fn storage_failures(machine: &Machine, headers: &HeaderMap) {
    sql(machine,"CREATE TRIGGER fail BEFORE DELETE ON local_sessions BEGIN SELECT RAISE(ABORT,'injected'); END").await;
    assert!(
        logout(State(machine.clone()), headers.clone())
            .await
            .is_err()
    );
    sql(machine, "DROP TRIGGER fail; UPDATE resources SET spec='{}'").await;
    assert!(
        configuration(State(machine.clone()), headers.clone())
            .await
            .is_err()
    );
    sql(
        machine,
        "UPDATE resources SET spec=NULL; INSERT INTO local_settings VALUES('model','bad')",
    )
    .await;
    assert!(
        configuration(State(machine.clone()), headers.clone())
            .await
            .is_err()
    );
    sql(machine, "DELETE FROM installation").await;
    assert!(
        configuration(State(machine.clone()), headers.clone())
            .await
            .is_err()
    );
    assert!(origin(machine, headers).await.is_err());
    assert!(cookie(machine, "value", 0).await.is_err());
    assert!(require_router(machine).await.is_err());
    assert!(
        enroll(
            State(machine.clone()),
            Json(crate::registry::Enrollment::from(&registration()))
        )
        .await
        .is_err()
    );
    machine.close().await;
    assert!(status(State(machine.clone())).await.is_err());
}
#[tokio::test]
async fn https_cookie_is_secure_and_unsupported_enrollment_is_rejected() {
    let (_dir, machine) = machine().await;
    sql(
        &machine,
        "UPDATE installation SET public_origin='https://machine.example'",
    )
    .await;
    assert!(
        cookie(&machine, "value", 20)
            .await
            .expect("cookie")
            .headers()[header::SET_COOKIE]
            .to_str()
            .expect("value")
            .contains("; Secure")
    );
    assert!(
        enroll(
            State(machine.clone()),
            Json(crate::registry::Enrollment::from(&registration()))
        )
        .await
        .is_err()
    );
    machine.close().await;
}
