//! dtrail library crate.
//!
//! Exposes parsing ([`session`]) and display ([`view`]) separately from the
//! TUI so they can be unit tested.

pub mod app;
pub mod clipboard;
pub mod conversation;
pub mod datefmt;
pub mod input;
pub mod scroll;
pub mod session;
pub mod tags;
pub mod terminal;
pub mod text;
pub mod ui;
pub mod view;
