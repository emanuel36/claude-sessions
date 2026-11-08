//! Core of `cst`, a sidebar for Claude Code sessions in tmux.

/// The version of this build, printed by `cst --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod claude;
pub mod journal;
pub mod paths;
pub mod shell;
pub mod state;
pub mod store;
pub mod term;
pub mod ui;
