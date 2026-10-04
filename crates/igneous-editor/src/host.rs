//! What the editor asks of the app it's in.

use std::path::PathBuf;

use igneous_markdown::LinkRef;

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
