#![allow(clippy::expect_used)]
use super::*;
use crate::tests::Fixture;
#[tokio::test]
async fn listeners_reject_public_hooks_bad_upstreams_and_occupied_addresses() {
    let f = Fixture::new().await;
    let address = "127.0.0.1:0".parse().expect("address");
    assert!(
        serve_until(
            f.platform.clone(),
            address,
            "0.0.0.0:0".parse().expect("address"),
            "http://127.0.0.1",
            async {}
        )
        .await
        .is_err()
    );
    assert!(
        serve_until(f.platform.clone(), address, address, "invalid", async {})
            .await
            .is_err()
    );
    let busy = tokio::net::TcpListener::bind(address)
        .await
        .expect("listener");
    assert!(
        serve_until(
            f.platform.clone(),
            address,
            busy.local_addr().expect("address"),
            "http://127.0.0.1",
            async {}
        )
        .await
        .is_err()
    );
    assert!(
        serve_until(
            f.platform.clone(),
            busy.local_addr().expect("address"),
            address,
            "http://127.0.0.1",
            async {}
        )
        .await
        .is_err()
    );
    serve_until(
        f.platform.clone(),
        address,
        address,
        "http://127.0.0.1",
        async { tokio::task::yield_now().await },
    )
    .await
    .expect("clean shutdown");
    f.close().await;
}
#[tokio::test]
async fn either_failed_service_or_shutdown_ends_the_lifecycle() {
    for side in [true, false] {
        let (stop, _) = tokio::sync::watch::channel(false);
        let result = drive(
            Box::pin(async move {
                if side {
                    Err(io::Error::other("public stopped"))
                } else {
                    Ok(())
                }
            }),
            Box::pin(async move {
                if side {
                    Ok(())
                } else {
                    Err(io::Error::other("private stopped"))
                }
            }),
            stop,
            Box::pin(std::future::pending()),
        )
        .await;
        assert!(result.is_err());
    }
    let (stop, _) = tokio::sync::watch::channel(false);
    assert!(
        drive(
            Box::pin(async { Ok(()) }),
            Box::pin(async { Ok(()) }),
            stop,
            Box::pin(std::future::pending())
        )
        .await
        .is_ok()
    );
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let copy = receiver.clone();
    assert!(
        drive(
            Box::pin(async move {
                stopping(receiver).await;
                Ok(())
            }),
            Box::pin(async move {
                stopping(copy).await;
                Ok(())
            }),
            stop,
            Box::pin(async {})
        )
        .await
        .is_ok()
    );
}
