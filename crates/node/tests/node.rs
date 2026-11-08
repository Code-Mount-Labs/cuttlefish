//! Node integration suite: journal, runtime, local control, and real processes.

mod common;
#[path = "suites/faults.rs"]
mod faults;
#[path = "suites/journal.rs"]
mod journal;
#[cfg(unix)]
#[path = "suites/providers.rs"]
mod providers;
#[path = "suites/runtime.rs"]
mod runtime;
#[path = "suites/survival.rs"]
mod survival;

#[cfg(unix)]
#[path = "suites/recovery.rs"]
mod recovery;
