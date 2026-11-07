//! Local node executable. See the library for runtime and journal contracts.

use clap::Parser;
use orchestrator_node::{
    NodeError,
    cli::{self, Cli},
};

fn main() -> Result<(), NodeError> {
    execute(Cli::parse(), || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    })
}

fn execute(
    args: Cli,
    build_runtime: fn() -> std::io::Result<tokio::runtime::Runtime>,
) -> Result<(), NodeError> {
    let runtime = build_runtime()?;
    let result = runtime.block_on(cli::run(args, std::env::current_exe));
    // Tokio's stdin uses an uncancellable blocking read. Do not let an open
    // input pipe prevent exit after an unrelated journal/supervision failure.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    result
}

#[cfg(test)]
#[path = "../tests/internal/bootstrap.rs"]
mod tests;
