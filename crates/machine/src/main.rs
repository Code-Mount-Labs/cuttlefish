//! Cuttlefish local installation executable.
use clap::Parser;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), cuttlefish_machine::Error> {
    cuttlefish_machine::cli::run(cuttlefish_machine::cli::Cli::parse()).await
}
