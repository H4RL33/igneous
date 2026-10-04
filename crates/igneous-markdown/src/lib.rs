//! Obsidian Flavoured Markdown for Igneous.
//!
//! [`parse`] turns a note into a [`Document`]: a flat list of syntax nodes plus
//! the links, tags, headings, tasks, callouts and block IDs found in it. Every
//! position is a byte offset into the note, so the editor, index and linter
//! can all point back into the same text.
//!
//! [`present::spans`] turns a document into the editor-agnostic presentation
//! model used by Live Preview: what to style, which syntax markers to hide, when
//! to reveal them, and which elements get replaced by widgets.

#![forbid(unsafe_code)]

pub mod edit;
pub mod frontmatter;
pub mod link;
mod parse;
pub mod present;
mod section;
pub mod text;

pub use edit::TextEdit;
pub use frontmatter::{Frontmatter, Value};
pub use link::{LinkRef, Subpath};
pub use parse::{
    BlockId, Callout, Document, Fold, Heading, Link, LinkKind, Node, NodeKind, Tag, Task, parse,
};
pub use section::subpath_range;

/// A byte range in a note.
pub type Span = std::ops::Range<usize>;
