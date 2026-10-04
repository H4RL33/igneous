//! Obsidian's search language and Bases, evaluated over [`NoteData`].
//!
//! - [`search`] parses queries such as `tag:#work "due date" -draft` and
//!   matches them against notes, returning byte ranges for highlighting.
//! - [`bases`] reads `.base` files, evaluates their filters and formulas, and
//!   produces the rows of a view. Changes to a view's columns, widths and
//!   sorting are written back as minimal text edits.
//!
//! The index fills a [`NoteData`] for each file in the vault. Nothing in this
//! crate touches the disk.

#![forbid(unsafe_code)]

pub mod bases;
mod note;
pub mod search;

pub use note::{LinkData, NoteData};
