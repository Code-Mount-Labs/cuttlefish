//! Versioned, transport-independent contracts shared by the server and nodes.
//!
//! IDs and validated values cannot be bypassed through deserialization. This
//! crate deliberately performs no I/O, UUID generation, or clock reads.
//!
//! Resource identities cannot be accidentally interchanged:
//!
//! ```compile_fail
//! use orchestrator_protocol::ids::{GroupId, NodeId};
//! let group: GroupId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
//! let node: NodeId = group;
//! ```
//!
//! Validated values cannot be constructed by bypassing their constructors:
//!
//! ```compile_fail
//! use orchestrator_protocol::values::MessageText;
//! let empty = MessageText(String::new());
//! ```

pub mod agent;
pub mod capabilities;
pub mod collaboration;
pub mod ids;
pub mod installation;
pub mod permission;
pub mod routing;
pub mod sync;
pub mod task;
pub mod values;
