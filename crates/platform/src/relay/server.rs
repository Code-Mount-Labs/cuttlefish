//! Lifecycle of the separate public gateway and private authorization listener.
use crate::{
    Platform,
    relay::{gateway, web},
};
use std::{io, net::SocketAddr};

/// Serve public JSON traffic and a loopback-only frps hook until shutdown.
///
/// # Errors
/// Rejects non-loopback hook addresses, invalid upstreams and listener failures.
pub async fn serve_until(
    platform: Platform,
    public: SocketAddr,
    private: SocketAddr,
    upstream: &str,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    serve(platform, public, private, upstream, Box::pin(shutdown)).await
}
async fn serve(
    platform: Platform,
    public: SocketAddr,
    private: SocketAddr,
    upstream: &str,
    shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
) -> io::Result<()> {
    if !private.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "relay hook must be loopback",
        ));
    }
    let router = gateway::router(platform.clone(), upstream).map_err(io::Error::other)?;
    let listener = tokio::net::TcpListener::bind(public).await?;
    let hook = tokio::net::TcpListener::bind(private).await?;
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let gateway = Box::pin(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopping(receiver.clone()))
            .await
    });
    let private_receiver = stop.subscribe();
    let private_router = web::plugin_router(platform.clone());
    let private = Box::pin(async move {
        axum::serve(hook, private_router)
            .with_graceful_shutdown(stopping(private_receiver))
            .await
    });
    let result = drive(gateway, private, stop, shutdown).await;
    platform.close().await;
    result
}
async fn stopping(mut receiver: tokio::sync::watch::Receiver<bool>) {
    let _ = receiver.wait_for(|stop| *stop).await;
}

type Service = std::pin::Pin<Box<dyn std::future::Future<Output = io::Result<()>> + Send>>;
async fn drive(
    public: Service,
    private: Service,
    stop: tokio::sync::watch::Sender<bool>,
    shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
) -> io::Result<()> {
    let servers = async {
        tokio::try_join!(public, private)?;
        Ok(())
    };
    tokio::pin!(servers);
    tokio::select! {
        result = &mut servers => result,
        () = shutdown => { stop.send_replace(true); servers.await }
    }
}
#[cfg(test)]
#[path = "../../tests/internal/relay_server.rs"]
mod tests;
