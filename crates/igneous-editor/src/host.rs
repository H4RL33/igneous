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
}

/// A host that knows nothing: every link exists, nothing opens.
pub struct NoHost;

impl Host for NoHost {}
