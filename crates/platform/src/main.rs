//! Hosted registry executable. No execution or router database credentials are
//! distributed through the public website.

use clap::Parser;
use cuttlefish_platform::{Platform, web};
use std::{io, net::IpAddr};

#[derive(Parser)]
#[command(version, about = "Cuttlefish account and installation registry")]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "PUBLIC_ORIGIN")]
    public_origin: String,
    #[arg(long, env = "HOST", default_value = "127.0.0.1")]
    host: IpAddr,
    #[arg(long, env = "PORT", default_value = "8080")]
    port: u16,
    #[arg(long, env = "RELAY_ORIGIN")]
    relay_origin: Option<String>,
    #[arg(long, env = "RELAY_PUBLIC_KEY", hide_env_values = true)]
    relay_public_key: Option<String>,
    #[arg(long, env = "RELAY_PRIVATE_KEY", hide_env_values = true)]
    relay_private_key: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let platform = Platform::open(&cli.database_url, &cli.public_origin)
        .await
        .map_err(io::Error::other)?;
    let platform = platform
        .configure_relay(
            cli.relay_origin.as_deref(),
            cli.relay_public_key.as_deref(),
            cli.relay_private_key.as_deref(),
        )
        .map_err(io::Error::other)?;
    let listener = tokio::net::TcpListener::bind((cli.host, cli.port)).await?;
    axum::serve(listener, web::router(platform))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
