//! What search and Bases know about a file.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use igneous_core::VaultPath;
use igneous_markdown::{Document, LinkKind, Value};

/// A link as the index resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkData {
    /// The target as written, without subpath or alias: `Note`, `folder/Note`.
    pub target: String,
    /// The file it resolves to, if it resolves.
    pub path: Option<VaultPath>,
}

impl LinkData {
    pub fn unresolved(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            path: None,
        }
    }
}

/// One file in the vault, as search and Bases see it.
///
/// Files other than notes (attachments, `.base` files) have no `text`,
/// `properties`, `tags` or links. As in Obsidian, search only finds them
/// through `file:` and `path:`, and Bases lists them unless a filter leaves
/// them out.
///
/// The parsed [`Document`] is only needed by search operators that look at a
/// note's structure (`line:`, `block:`, `section:`, `task:`, `tag:`). It's
/// parsed on first use, or the index can hand over one it already has with
/// [`NoteData::with_document`].
#[derive(Debug, Clone)]
pub struct NoteData {
    pub path: VaultPath,
    /// The note's text with `\n` line endings. Empty for other files.
    pub text: Arc<str>,
    /// Size on disk in bytes.
    pub size: u64,
    /// Creation time, in milliseconds since the Unix epoch.
    pub ctime: i64,
    /// Modification time, in milliseconds since the Unix epoch.
    pub mtime: i64,
    /// Frontmatter properties.
    pub properties: BTreeMap<String, Value>,
    /// Inline and frontmatter tags, without `#`, in order of appearance.
    pub tags: Vec<String>,
    /// Outgoing links, including those in frontmatter. Embeds aren't included.
    pub links: Vec<LinkData>,
    pub embeds: Vec<LinkData>,
    /// Files linking here. Bases' `file.backlinks` needs it; search doesn't.
    pub backlinks: Vec<VaultPath>,
    document: OnceLock<Document>,
}

impl NoteData {
    /// A file with nothing known about it but its path.
    pub fn new(path: VaultPath) -> Self {
        Self {
            path,
            text: Arc::from(""),
            size: 0,
            ctime: 0,
            mtime: 0,
            properties: BTreeMap::new(),
            tags: Vec::new(),
            links: Vec::new(),
            embeds: Vec::new(),
            backlinks: Vec::new(),
            document: OnceLock::new(),
        }
    }

    /// A note with its properties, tags and (unresolved) links taken from
    /// `text`. The index resolves links itself; this is mostly for tests.
    pub fn from_text(path: VaultPath, text: impl Into<Arc<str>>) -> Self {
        let text: Arc<str> = text.into();
        let doc = igneous_markdown::parse(&text);
        let mut note = Self::new(path);
        note.size = text.len() as u64;
        note.properties = doc
            .frontmatter
            .iter()
            .flat_map(|fm| &fm.entries)
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect();
        note.tags = collect_tags(&doc, &note.properties);
        for link in &doc.links {
            if matches!(link.kind, LinkKind::Autolink | LinkKind::Url)
                || link.reference.target.is_empty()
                || link.reference.is_external()
            {
                continue;
            }
            let data = LinkData::unresolved(link.reference.target.clone());
            if link.embed {
                note.embeds.push(data);
            } else {
                note.links.push(data);
            }
        }
        note.text = text;
        note.with_document(doc)
    }

    /// Hands over an already parsed document of `text`.
    pub fn with_document(self, doc: Document) -> Self {
        let _ = self.document.set(doc);
        self
    }

    /// The parsed note, parsing it on first use.
    pub fn document(&self) -> &Document {
        self.document
            .get_or_init(|| igneous_markdown::parse(&self.text))
    }

    /// Whether this is a Markdown note (as opposed to an attachment).
    pub fn is_note(&self) -> bool {
        self.path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    }

    /// A property by name, matching case-insensitively if there's no exact
    /// match (Obsidian treats `Status` and `status` as one property).
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.get(key).or_else(|| {
            let key = key.to_lowercase();
            self.properties
                .iter()
                .find(|(k, _)| k.to_lowercase() == key)
                .map(|(_, v)| v)
        })
    }
}

/// Inline tags plus the frontmatter `tags` property, without `#` and without
/// duplicates.
fn collect_tags(doc: &Document, properties: &BTreeMap<String, Value>) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut push = |tag: &str| {
        let tag = tag.trim().trim_start_matches('#');
        if !tag.is_empty() && !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_owned());
        }
    };
    if let Some(value) = properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("tags"))
        .map(|(_, v)| v)
    {
        for item in value.string_list() {
            // `tags: one, two` and `tags: one two` are both lists to Obsidian.
            for tag in item.split([',', ' ']) {
                push(tag);
            }
        }
    }
    for tag in &doc.tags {
        push(&tag.name);
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_collects_tags_and_links() {
        let path = VaultPath::new("Notes/Plan.md").unwrap();
        let note = NoteData::from_text(
            path,
            "---\ntags: [project, '#rust']\nup: \"[[Home]]\"\n---\nSee [[Roadmap|the roadmap]] #project #gtk/4\n![[diagram.png]] https://example.com\n",
        );
        assert_eq!(note.tags, ["project", "rust", "gtk/4"]);
        let mut links: Vec<_> = note.links.iter().map(|l| l.target.as_str()).collect();
        links.sort_unstable();
        assert_eq!(links, ["Home", "Roadmap"]);
        assert_eq!(note.embeds[0].target, "diagram.png");
        assert!(note.is_note());
        assert!(note.property("TAGS").is_some());
    }
}
