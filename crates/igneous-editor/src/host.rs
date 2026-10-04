//! What the editor asks of the app it's in.

use std::path::PathBuf;

use igneous_markdown::LinkRef;

use crate::properties::PropertyKind;

/// A note shown inside another (`![[Note]]`, `![[Note#Heading]]`).
pub struct Embed {
    pub title: String,
    /// The embedded Markdown: the whole note, or just the heading's section
    /// or the block.
    pub text: String,
}

/// A note offered when completing `[[`.
pub struct NoteName {
    /// What goes in the link: the name, or the path if the name is shared.
    pub link: String,
    pub title: String,
    /// The folder, shown beside the name.
    pub detail: String,
    /// Set when this entry is one of the note's aliases.
    pub alias: Option<String>,
}

pub trait Host {
    /// Whether a link points at something that exists. Unresolved links are
    /// styled differently. Called for every link after every change, so it
    /// must be quick.
    fn link_exists(&self, _link: &LinkRef) -> bool {
        true
    }

    /// The file an image embed shows.
    fn image_path(&self, _target: &str) -> Option<PathBuf> {
        None
    }

    /// The content of a note embed.
    fn embed(&self, _link: &LinkRef) -> Option<Embed> {
        None
    }

    /// Follows a link (Ctrl+click, middle-click, or a click in Reading mode).
    fn open_link(&self, _link: &LinkRef, _new_tab: bool) {}

    /// Notes (and aliases) to offer after `[[`.
    fn note_names(&self) -> Vec<NoteName> {
        Vec::new()
    }

    /// The headings of the note `target` refers to.
    fn headings(&self, _target: &str) -> Vec<String> {
        Vec::new()
    }

    /// The block IDs of the note `target` refers to, with their text.
    fn blocks(&self, _target: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    /// The vault's tags, with how many notes use each.
    fn tags(&self) -> Vec<(String, usize)> {
        Vec::new()
    }

    /// The type the vault gives a property (set by the user or inferred
    /// from every note). `None` to judge by this note's value alone.
    fn property_kind(&self, _key: &str) -> Option<PropertyKind> {
        None
    }

    /// Makes `key` a property of this kind throughout the vault.
    fn set_property_kind(&self, _key: &str, _kind: PropertyKind) {}

    /// Property keys used in the vault, most common first.
    fn property_keys(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A host that knows nothing: every link exists, nothing opens.
pub struct NoHost;

impl Host for NoHost {}
