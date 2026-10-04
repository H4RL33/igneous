//! The metadata cache behind backlinks, tags, properties and search.
//!
//! [`Index`] keeps what every note links to, its tags, properties, aliases,
//! headings, block IDs, tasks and text in SQLite (with an FTS5 table for
//! search), in the vault's cache folder. It's a disposable cache: files are
//! the truth, and [`Index::reconcile`] rebuilds whatever changed since the
//! last run.
//!
//! Links are stored as written. They're resolved when asked about, against
//! the [`FileSet`] of every file in the vault, so adding or removing a file
//! never leaves stored resolutions stale.
//!
//! [`refactor`] plans the edits that keep links working when files move and
//! that rename tags across the vault.

#![forbid(unsafe_code)]

mod error;
mod extract;
mod index;
pub mod refactor;
pub mod resolve;
mod schema;
pub mod types;
mod value;

pub use error::IndexError;
pub use index::{
    BlockEntry, EdgeTarget, GraphEdge, HeadingEntry, Index, LinkHit, Mention, NoteEntry, NoteRow,
    OutLink, PropertyInfo, ReconcileStats, TaskEntry, Unresolved,
};
pub use refactor::{FileEdits, RefactorPlan};
pub use resolve::{FileSet, Method};
