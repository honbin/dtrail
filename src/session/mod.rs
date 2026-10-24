//! Session discovery, parsing, and the internal model.
//! This subtree has no TUI dependency so it can be unit tested on its own.

pub mod claude;
pub mod codex;
mod content;
pub mod cursor;
pub mod discovery;
pub mod model;
mod scan;

pub use discovery::discover;
pub use model::{
    Agent, Block, ParseStats, Record, RecordKind, Session, SessionEntry, ToolResultContent,
};
