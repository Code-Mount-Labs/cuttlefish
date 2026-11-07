//! Operator CLI integration tests against isolated `PostgreSQL` schemas.
#![allow(clippy::expect_used)]
#[cfg(unix)]
#[path = "suites/cli.rs"]
mod cli;
mod common;
#[path = "../../protocol/tests/fixtures/mod.rs"]
mod data;
