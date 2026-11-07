//! Node integration suite: journal, runtime, local control, and real processes.

mod common;
#[path = "suites/faults.rs"]
mod faults;
#[path = "suites/journal.rs"]
mod journal;
#[path = "suites/runtime.rs"]
mod runtime;
#[path = "suites/survival.rs"]
mod survival;
