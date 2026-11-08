//! Shared test executable for library boundaries and internal fault injection.
#![allow(clippy::expect_used)]
#[path = "../suites/collaboration.rs"]
mod collaboration;
#[path = "../suites/collaboration_faults.rs"]
mod collaboration_faults;
#[path = "../common/mod.rs"]
pub(crate) mod common;
#[path = "../../../protocol/tests/fixtures/mod.rs"]
pub(crate) mod data;
#[path = "../suites/delivery.rs"]
mod delivery;
#[path = "../suites/delivery_faults.rs"]
mod delivery_faults;
#[path = "../suites/http.rs"]
mod http;
#[path = "../suites/routing.rs"]
pub(crate) mod routing;
#[path = "../suites/routing_faults.rs"]
mod routing_faults;
#[path = "../suites/selector.rs"]
mod selector;
#[path = "../suites/store.rs"]
mod store;
#[path = "../suites/super_agent.rs"]
mod super_agent;
#[path = "../suites/sync.rs"]
mod sync;

#[path = "../suites/collaboration_storage.rs"]
mod collaboration_storage;

#[path = "../suites/super_agent_faults.rs"]
mod super_agent_faults;

#[path = "../suites/chat.rs"]
mod chat;

#[path = "../suites/attachments.rs"]
mod attachments;

#[path = "../suites/activity.rs"]
mod activity;

#[path = "../suites/automations.rs"]
mod automations;

#[path = "../suites/routines.rs"]
mod routines;

#[path = "../suites/agent_tools.rs"]
pub(crate) mod agent_tools;
