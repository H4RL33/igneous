//! What the editor asks of the app it's in.

use std::path::{Path, PathBuf};

use igneous_markdown::LinkRef;

use crate::properties::PropertyKind;

/// A note shown inside another (`![[Note]]`, `![[Note#Heading]]`).
pub struct Embed {
    pub title: String,
    /// The embedded Markdown: the whole note, or just the heading's section
    /// or the block.
    pub text: String,
}

/// A line in another note that links here, for the Linked Mentions
/// section at the end of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub title: String,
    /// The linking note's vault path.
    pub path: String,
    pub line: String,
    /// Where in that note, as a byte offset.
    pub at: usize,
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

/// A base shown in a note.
pub enum BaseEmbed<'a> {
    /// `![[File.base]]` or `![[File.base#View]]`.
    File(&'a LinkRef),
    /// The body of a ` ```base ` block.
    Block(&'a str),
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

    /// Opens another note at a byte offset (from Linked Mentions).
    fn open_mention(&self, _mention: &Mention) {}

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

    /// The property names used in the vault, as a menu of
    /// [`property_name_item`](crate::property_name_item)s that every note's
    /// Add Property shares. The host appends to it as names come into use.
    fn property_names(&self) -> Option<gtk::gio::MenuModel> {
        None
    }

    /// `key` has just been made a property of a note.
    fn add_property_name(&self, _key: &str) {}

    /// Rescans the vault and remakes [`property_names`](Self::property_names)
    /// from the names its notes use now.
    fn refresh_property_names(&self) {}

    /// Saves pasted data (an image) as an attachment named like `name`,
    /// returning the embed to insert, e.g. `![[Pasted image 1.png]]`.
    fn save_attachment(&self, _name: &str, _bytes: &[u8]) -> Option<String> {
        None
    }

    /// Copies a dropped or pasted file into the vault as an attachment
    /// (or links it, if it's already in the vault), returning the embed or
    /// link to insert.
    fn attach_file(&self, _path: &Path) -> Option<String> {
        None
    }

    /// Fixes the problems `rule` found (the Fix button on an underline).
    fn fix(&self, _rule: &str) {}

    /// Whether [`Host::base_widget`] can show bases. Without it, ` ```base `
    /// blocks stay code and `![[File.base]]` embeds show their source.
    fn embeds_bases(&self) -> bool {
        false
    }

    /// A widget showing a base's results, for embeds and ` ```base ` blocks.
    fn base_widget(&self, _base: BaseEmbed<'_>) -> Option<gtk::Widget> {
        None
    }
}

/// A host that knows nothing: every link exists, nothing opens.
pub struct NoHost;

impl Host for NoHost {}
