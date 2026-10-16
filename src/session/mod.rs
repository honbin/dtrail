//! Session discovery, parsing, and the internal model.
//! This subtree has no TUI dependency so it can be unit tested on its own.

pub mod claude;
pub mod codex;
pub mod discovery;
pub mod model;

pub use discovery::{Agent, SessionEntry, discover};
pub use model::{Block, ParseStats, Record, RecordKind, Session, ToolResultContent};
