#![deny(unsafe_code, unused_must_use)]
//! git-rs: a git reimplementation in Rust.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diff;
pub mod error;
pub mod ignore;
pub mod index;
pub mod object;
pub mod refs;
pub mod revwalk;
pub mod store;
pub mod worktree;
