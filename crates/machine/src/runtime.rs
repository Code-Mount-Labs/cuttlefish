//! Installation service lifecycle. Routing, directory reporting and execution
//! remain independent; a registry outage never cancels ongoing local work.
use crate::{
    Error,
    registry::{Client, Enrollment},
    state::Machine,
};
use orchestrator_node::{
    model::NodeIdentity, process::LocalLauncher, runtime::NodeRuntime, sync::Remote,
};
use orchestrator_protocol::{
    capabilities::{Capability, Platform},
    ids::NodeId,
    sync::NodeProfile,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Run the local dashboard and optional execution agent until shutdown.
///
/// # Errors
/// Reports listener/startup failure. Transient directory and router failures retry.
pub async fn serve(
    machine: Machine,
    listen: std::net::SocketAddr,
    worker: PathBuf,
) -> Result<(), Error> {
    serve_until(machine, listen, worker, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Run until the supplied shutdown signal, including orderly task cancellation.
///
/// # Errors
/// Reports listener or HTTP service failures.
pub async fn serve_until(
    machine: Machine,
    listen: std::net::SocketAddr,
    worker: PathBuf,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), Error> {
    serve_with_shutdown(
        machine,
        listen,
        worker,
        Box::pin(shutdown),
        tokio::net::TcpListener::local_addr,
    )
    .await
}
async fn serve_with_shutdown(
    machine: Machine,
    listen: std::net::SocketAddr,
    worker: PathBuf,
    shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    address_of: fn(&tokio::net::TcpListener) -> std::io::Result<std::net::SocketAddr>,
) -> Result<(), Error> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let address = address_of(&listener)?;
    let loopback = if address.is_ipv6() {
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    };
    machine
        .set_local_endpoint(&format!(
            "http://{}",
            std::net::SocketAddr::new(loopback, address.port())
        ))
        .await?;
    let remote = tokio::spawn(recover_remote(machine.clone()));
    let ready = Arc::new(AtomicBool::new(false));
    let reporting = tokio::spawn(report(machine.clone(), ready.clone()));
    let recovering = tokio::spawn(recover(machine.clone()));
    let execution = tokio::spawn(agent(machine.clone(), worker, ready));
    let result = axum::serve(listener, crate::web::router(machine.clone()))
        .with_graceful_shutdown(shutdown)
        .await;
    remote.abort();
    reporting.abort();
    recovering.abort();
    execution.abort();
    let _ = tokio::join!(reporting, recovering, execution, remote);
    machine.close().await;
    result.map_err(Error::from)
}
async fn recover(machine: Machine) {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Ok(status) = machine.status().await
            && status.state == "provisioning"
            && machine.recover().await.is_err()
        {
            eprintln!("Database recovery needs attention in the local dashboard.");
        }
    }
}

async fn report(machine: Machine, ready: Arc<AtomicBool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if machine
            .advertise(ready.load(Ordering::SeqCst))
            .await
            .is_err()
        {
            eprintln!("Directory update unavailable; local execution is unchanged.");
        }
    }
}
async fn agent(machine: Machine, worker: PathBuf, ready: Arc<AtomicBool>) {
    let result = run_agent(&machine, worker, ready.clone()).await;
    ready.store(false, Ordering::SeqCst);
    if result.is_err() {
        eprintln!("Agent startup needs attention; the local dashboard remains available.");
    }
}
async fn run_agent(
    machine: &Machine,
    worker: PathBuf,
    ready: Arc<AtomicBool>,
) -> Result<(), Error> {
    let identity = machine.identity().await?;
    if !identity.registration.role.has_agent() {
        return Ok(());
    }
    if !worker.is_file() {
        return Err(Error::Dependency("specialist_worker_binary_missing"));
    }
    let node = NodeIdentity {
        group_id: identity.registration.group_id,
        node_id: NodeId::from(identity.registration.installation_id),
    };
    let mut runtime = NodeRuntime::open(
        &machine.directory().join("agent"),
        node,
        LocalLauncher::new(worker),
    )
    .await
    .map_err(|_| Error::Dependency("agent_journal_unavailable"))?;
    let profile = NodeProfile {
        platform: native_platform(std::env::consts::OS),
        capabilities: Capability::native(),
        capacity: 4,
    };
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut enrolled = false;
    loop {
        interval.tick().await;
        let Ok(origin) = machine.router_origin().await else {
            continue;
        };
        if !enrolled {
            let result = Client::new(&origin)?
                .post::<_, serde_json::Value>(
                    "/api/node/enroll",
                    None,
                    &Enrollment::from(&identity.registration),
                )
                .await;
            if result.is_err() {
                continue;
            }
            enrolled = true;
        }
        let remote = Remote::new(&origin, &identity.registration.credential)
            .map_err(|_| Error::Dependency("invalid_router_configuration"))?;
        ready.store(
            remote.cycle(&mut runtime, &profile).await.is_ok(),
            Ordering::SeqCst,
        );
    }
}
fn native_platform(os: &str) -> Platform {
    match os {
        "macos" => Platform::MacOs,
        "windows" => Platform::Windows,
        _ => Platform::Linux,
    }
}

#[cfg(test)]
#[path = "../tests/internal/runtime.rs"]
mod tests;

async fn recover_remote(machine: Machine) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if machine.recover_remote().await.is_err() {
            eprintln!("Remote access needs attention; local work continues.");
        }
    }
}
