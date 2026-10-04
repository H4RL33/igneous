//! Git sync for Igneous, in the style of obsidian-git, built on the `git`
//! command rather than a library (PROJECT.md D6). Running `git` itself means
//! clean/smudge filters, hooks, signing, SSH configuration and credential
//! helpers all behave exactly as they do in a terminal.
//!
//! - [`Git`] finds a repository and runs `git` in it, without ever waiting on
//!   a terminal prompt.
//! - [`status`] parses `git status --porcelain=v2`.
//! - [`sync`] runs commit-and-sync and pull-only passes.
//! - [`template`] renders commit messages.
//! - [`conflicts`] finds and resolves conflict blocks in a file.
//!
//! Some operations are missing on purpose: there's no force-push, no
//! `reset --hard`, and no automatic conflict resolution. [`Git::discard`] is
//! the only call that throws work away, and the app always asks first.

#![forbid(unsafe_code)]

mod cli;
pub mod conflicts;
mod error;
pub mod status;
pub mod sync;
pub mod template;

pub use cli::{Commit, Git, MIN_VERSION};
pub use error::{FailureKind, GitError};
pub use status::{Entry, InProgress, RepoStatus};
pub use sync::{Phase, SyncKind, SyncOptions, SyncReport};
