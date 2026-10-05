//! Finding the file a link points to.
//!
//! A target is tried, in order, as Obsidian does:
//!
//! 1. as a path from the vault root, with or without `.md`;
//! 2. as a path from the linking note's folder (`../` included);
//! 3. as a file name: a note's name without `.md`, or any other file's full
//!    name, compared case-insensitively after Unicode normalisation. A target
//!    with folders in it (`Projects/Note`) must match the end of the path.
//!
//! When a name matches several files, the one in the linking note's folder
//! wins, then the one with the shortest path, then the first alphabetically.

use std::collections::{BTreeSet, HashMap};

use igneous_core::VaultPath;
use igneous_core::path::loose_key;
use igneous_markdown::LinkRef;

/// How a link found its file. Renames use this to rewrite links in the style
/// they were written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// No target: the note links to itself (`[[#Heading]]`).
    SameNote,
    /// A path from the vault root.
    Exact,
    /// A path from the linking note's folder.
    Relative,
    /// A file name, perhaps with some of its folders.
    Name,
}

/// Every file in a vault, arranged for resolving links.
#[derive(Debug, Clone, Default)]
pub struct FileSet {
    paths: BTreeSet<VaultPath>,
    /// Loose key of the full path → files.
    by_path: HashMap<String, Vec<VaultPath>>,
    /// [`name_key`] → files.
    by_name: HashMap<String, Vec<VaultPath>>,
}

/// Whether a file is a note (Markdown).
pub fn is_note(path: &VaultPath) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// What a link must name to reach `path`: a note's name without `.md`, or
/// any other file's full name, case-folded.
pub fn name_key(path: &VaultPath) -> String {
    if is_note(path) {
        loose_key(path.stem())
    } else {
        loose_key(path.file_name())
    }
}

/// The name a link target refers to: its last component without `.md`,
/// case-folded. Comparable with [`name_key`].
pub fn target_key(target: &str) -> String {
    let last = target.trim().rsplit('/').next().unwrap_or("");
    loose_key(strip_md(last))
}

fn strip_md(s: &str) -> &str {
    match s.len().checked_sub(3) {
        Some(i) if s.is_char_boundary(i) && s[i..].eq_ignore_ascii_case(".md") => &s[..i],
        _ => s,
    }
}

fn has_md(s: &str) -> bool {
    strip_md(s).len() != s.len()
}

impl FileSet {
    pub fn new(paths: impl IntoIterator<Item = VaultPath>) -> Self {
        let mut set = Self::default();
        for path in paths {
            set.insert(path);
        }
        set
    }

    pub fn insert(&mut self, path: VaultPath) {
        if self.paths.contains(&path) {
            return;
        }
        self.by_path
            .entry(path.loose_key())
            .or_default()
            .push(path.clone());
        self.by_name
            .entry(name_key(&path))
            .or_default()
            .push(path.clone());
        self.paths.insert(path);
    }

    pub fn remove(&mut self, path: &VaultPath) {
        if !self.paths.remove(path) {
            return;
        }
        for (map, key) in [
            (&mut self.by_path, path.loose_key()),
            (&mut self.by_name, name_key(path)),
        ] {
            if let Some(list) = map.get_mut(&key) {
                list.retain(|p| p != path);
                if list.is_empty() {
                    map.remove(&key);
                }
            }
        }
    }

    pub fn contains(&self, path: &VaultPath) -> bool {
        self.paths.contains(path)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Every file, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = &VaultPath> {
        self.paths.iter()
    }

    /// Files inside `folder`, at any depth.
    pub fn under(&self, folder: &VaultPath) -> impl Iterator<Item = &VaultPath> {
        let folder = folder.clone();
        self.paths
            .iter()
            .filter(move |p| **p != folder && p.starts_with(&folder))
    }

    /// Files a link could reach by `name` (see [`name_key`]).
    pub fn named(&self, key: &str) -> &[VaultPath] {
        self.by_name.get(key).map_or(&[], Vec::as_slice)
    }

    /// The file `reference` points to from the note at `from`.
    pub fn resolve(&self, reference: &LinkRef, from: &VaultPath) -> Option<VaultPath> {
        self.resolve_target(&reference.target, from)
            .map(|(path, _)| path)
    }

    /// Resolves a link target as written, and says how it matched.
    pub fn resolve_target(&self, target: &str, from: &VaultPath) -> Option<(VaultPath, Method)> {
        let target = target.trim();
        if target.is_empty() {
            return Some((from.clone(), Method::SameNote));
        }

        // A path from the vault root.
        if let Ok(path) = VaultPath::new(target.trim_start_matches('/'))
            && let Some(found) = self.exact(path.as_str())
        {
            return Some((found, Method::Exact));
        }

        // A path from the linking note's folder.
        if !target.starts_with('/')
            && let Some(path) = join_relative(from.parent().as_ref(), target)
            && let Some(found) = self.exact(path.as_str())
        {
            return Some((found, Method::Relative));
        }

        // A name, perhaps with folders that must match the end of the path.
        let candidates = self.named(&target_key(target));
        let suffix = target
            .trim_start_matches('/')
            .trim_start_matches("./")
            .to_owned();
        let candidates: Vec<&VaultPath> = if suffix.contains('/') {
            let want = loose_key(strip_md(&suffix));
            candidates
                .iter()
                .filter(|p| {
                    let key = if is_note(p) {
                        loose_key(strip_md(p.as_str()))
                    } else {
                        p.loose_key()
                    };
                    key == want || key.ends_with(&format!("/{want}"))
                })
                .collect()
        } else {
            candidates.iter().collect()
        };
        let best = match candidates.as_slice() {
            [] => return None,
            [only] => (*only).clone(),
            several => {
                let folder = from.parent();
                (*several
                    .iter()
                    .min_by(|a, b| {
                        let same = |p: &VaultPath| p.parent() != folder;
                        same(a)
                            .cmp(&same(b))
                            .then_with(|| a.components().count().cmp(&b.components().count()))
                            .then_with(|| a.as_str().len().cmp(&b.as_str().len()))
                            .then_with(|| a.as_str().cmp(b.as_str()))
                    })
                    .expect("several candidates"))
                .clone()
            }
        };
        Some((best, Method::Name))
    }

    /// The file at `path` (case-insensitively), trying `path.md` too.
    fn exact(&self, path: &str) -> Option<VaultPath> {
        let lookup = |p: &str| -> Option<VaultPath> {
            let found = self.by_path.get(&loose_key(p))?;
            found
                .iter()
                .find(|f| f.as_str() == p)
                .or(found.first())
                .cloned()
        };
        lookup(path).or_else(|| {
            if has_md(path) {
                None
            } else {
                lookup(&format!("{path}.md"))
            }
        })
    }

    /// The shortest wikilink target that reaches `to`, as Obsidian writes
    /// it: the name alone if no other file has it, else the full path. Notes
    /// drop their `.md`.
    pub fn link_text(&self, to: &VaultPath) -> String {
        let unique = self.named(&name_key(to)).len() <= 1;
        match (unique, is_note(to)) {
            (true, true) => to.stem().to_owned(),
            (true, false) => to.file_name().to_owned(),
            (false, true) => strip_md(to.as_str()).to_owned(),
            (false, false) => to.as_str().to_owned(),
        }
    }
}

/// `target` taken from `folder` (`None` is the vault root), with `.` and
/// `..` worked out. `None` if it climbs out of the vault.
pub fn join_relative(folder: Option<&VaultPath>, target: &str) -> Option<VaultPath> {
    let mut parts: Vec<&str> = folder.map(|f| f.components().collect()).unwrap_or_default();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    VaultPath::new(&parts.join("/")).ok()
}

/// The relative path from the folder `from` (`None` is the root) to `to`.
pub fn relative_path(from: Option<&VaultPath>, to: &VaultPath) -> String {
    let from: Vec<&str> = from.map(|f| f.components().collect()).unwrap_or_default();
    let to_parts: Vec<&str> = to.components().collect();
    let common = from
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out: Vec<&str> = vec![".."; from.len() - common];
    out.extend(&to_parts[common..]);
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    fn set(paths: &[&str]) -> FileSet {
        FileSet::new(paths.iter().map(|s| p(s)))
    }

    #[test]
    fn keys() {
        assert_eq!(target_key("Folder/Note.md"), "note");
        assert_eq!(target_key("Note.MD"), "note");
        assert_eq!(target_key("img.PNG"), "img.png");
        assert_eq!(name_key(&p("a/Note.md")), "note");
        assert_eq!(name_key(&p("a/img.PNG")), "img.png");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            join_relative(Some(&p("a/b")), "../c/N.md"),
            Some(p("a/c/N.md"))
        );
        assert_eq!(join_relative(None, "../x"), None);
        assert_eq!(relative_path(Some(&p("a/b")), &p("a/c/N.md")), "../c/N.md");
        assert_eq!(relative_path(None, &p("a/N.md")), "a/N.md");
        assert_eq!(relative_path(Some(&p("a")), &p("a/N.md")), "N.md");
    }

    #[test]
    fn under_lists_a_folders_files() {
        let files = set(&["a.md", "a/b.md", "a/c/d.md", "a b.md", "ab.md"]);
        let under: Vec<_> = files.under(&p("a")).map(VaultPath::as_str).collect();
        assert_eq!(under, ["a/b.md", "a/c/d.md"]);
    }

    #[test]
    fn link_text_is_shortest_unique() {
        let files = set(&["a/Note.md", "b/Note.md", "Other.md", "img.png"]);
        assert_eq!(files.link_text(&p("a/Note.md")), "a/Note");
        assert_eq!(files.link_text(&p("Other.md")), "Other");
        assert_eq!(files.link_text(&p("img.png")), "img.png");
    }
}
