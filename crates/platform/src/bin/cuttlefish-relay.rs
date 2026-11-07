//! Separate relay gateway executable. Execution databases are never accessed here.
use clap::Parser;
use cuttlefish_platform::{
    Platform,
    relay::{keys::Keys, server},
};
use std::{io, net::SocketAddr};
#[derive(Parser)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "PUBLIC_ORIGIN")]
    issuer: String,
    #[arg(long, env = "RELAY_ORIGIN")]
    relay_origin: String,
    #[arg(long, env = "RELAY_PUBLIC_KEY", hide_env_values = true)]
    public_key: String,
    #[arg(long, default_value = "127.0.0.1:7080")]
    listen: SocketAddr,
    #[arg(long, default_value = "127.0.0.1:7081")]
    hook: SocketAddr,
    #[arg(long, default_value = "http://127.0.0.1:7001")]
    upstream: String,
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let keys = Keys::new(&cli.issuer, &cli.relay_origin, &cli.public_key, None)
        .map_err(io::Error::other)?;
    let platform = Platform::open(&cli.database_url, &cli.issuer)
        .await
        .map_err(io::Error::other)?
        .with_relay(keys);
    server::serve_until(platform, cli.listen, cli.hook, &cli.upstream, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
