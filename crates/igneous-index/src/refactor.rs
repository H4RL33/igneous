//! Plans for changes across the vault: moving or renaming files and folders
//! without breaking links to them, and renaming tags.
//!
//! A plan only says what to change; the app shows it, applies it and can
//! undo it. Every edit replaces just the part of a link (or tag) that must
//! change, so aliases, headings, block references, embeds and the way a link
//! was written all survive.

use std::collections::{BTreeSet, HashMap, HashSet};

use igneous_core::VaultPath;
use igneous_markdown::{Link, LinkKind, Span, TextEdit};

use crate::extract::frontmatter_tags;
use crate::resolve::{FileSet, Method, is_note, name_key, relative_path};
use crate::{Index, IndexError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefactorPlan {
    /// Files to move, as `(from, to)`. Empty for tag renames.
    pub moves: Vec<(VaultPath, VaultPath)>,
    /// Notes to edit, sorted by path.
    pub files: Vec<FileEdits>,
}

/// The edits to one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdits {
    /// Where the note is before any move.
    pub path: VaultPath,
    /// Where it is after the moves (the same for most notes).
    pub path_after: VaultPath,
    /// The text the edits apply to, as indexed. If the note now reads
    /// differently, plan again rather than applying these edits.
    pub text: String,
    pub edits: Vec<TextEdit>,
}

impl FileEdits {
    /// The note's text with the edits applied.
    pub fn apply(&self) -> String {
        igneous_markdown::edit::apply(&self.text, &self.edits)
    }
}

impl RefactorPlan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// How many links or tags change.
    pub fn edit_count(&self) -> usize {
        self.files.iter().map(|f| f.edits.len()).sum()
    }
}

/// The edits that keep every link working when the file or folder `from`
/// becomes `to`.
///
/// Each changed link keeps its form: a wikilink stays a wikilink, a name
/// stays a name if it's still unique (else it becomes the full path), a path
/// from the vault root stays one, and a relative Markdown path is worked out
/// again from where the linking note will be. Links inside moved notes are
/// covered too, as are links in frontmatter.
pub fn plan_rename(
    index: &Index,
    from: &VaultPath,
    to: &VaultPath,
) -> Result<RefactorPlan, IndexError> {
    let before = index.files();
    let moves: Vec<(VaultPath, VaultPath)> = if before.contains(from) {
        vec![(from.clone(), to.clone())]
    } else {
        before
            .under(from)
            .filter_map(|p| {
                let rest = &p.as_str()[from.as_str().len()..];
                Some((
                    p.clone(),
                    VaultPath::new(&format!("{}{rest}", to.as_str())).ok()?,
                ))
            })
            .collect()
    };
    let mut plan = RefactorPlan {
        moves: moves.clone(),
        files: Vec::new(),
    };
    if moves.is_empty() || from == to {
        return Ok(plan);
    }
    let moved: HashMap<&VaultPath, &VaultPath> = moves.iter().map(|(a, b)| (a, b)).collect();
    let mut after = before.clone();
    for (old, new) in &moves {
        after.remove(old);
        after.insert(new.clone());
    }

    // Notes linking to a moved file, notes whose links a new name could
    // capture, and the moved notes themselves (for their relative links).
    let mut sources: BTreeSet<VaultPath> = BTreeSet::new();
    let keys: HashSet<String> = moves
        .iter()
        .flat_map(|(old, new)| [name_key(old), name_key(new)])
        .collect();
    for key in keys {
        sources.extend(index.sources_linking(&key)?);
    }
    sources.extend(moves.iter().map(|(old, _)| old.clone()).filter(is_note));

    for source in sources {
        let Some(text) = index.text(&source)? else {
            continue;
        };
        let source_after = moved.get(&source).map_or(source.clone(), |p| (*p).clone());
        let doc = igneous_markdown::parse(&text);
        let mut edits = Vec::new();
        for link in &doc.links {
            if !matches!(link.kind, LinkKind::Wiki | LinkKind::Markdown)
                || link.reference.is_external()
                || link.reference.target.trim().is_empty()
            {
                continue;
            }
            let target = link.reference.target.trim();
            let Some((old_target, method)) = before.resolve_target(target, &source) else {
                continue;
            };
            let new_target = moved
                .get(&old_target)
                .map_or(old_target.clone(), |p| (*p).clone());
            if after.resolve_target(target, &source_after).map(|r| r.0) == Some(new_target.clone())
            {
                continue;
            }
            let Some((span, syntax)) = target_span(&text, link) else {
                continue;
            };
            // Wikilinks are names or paths from the root, never relative,
            // even when the target happens to sit beside the note.
            let method = match (syntax, method) {
                (Syntax::Wiki, Method::Relative) => Method::Name,
                _ => method,
            };
            if let Some(written) =
                new_target_text(target, method, syntax, &new_target, &source_after, &after)
            {
                edits.push(TextEdit::replace(span, written));
            }
        }
        if !edits.is_empty() {
            plan.files.push(FileEdits {
                path: source,
                path_after: source_after,
                text,
                edits,
            });
        }
    }
    Ok(plan)
}

/// The edits that rename the tag `old` to `new` everywhere, nested tags
/// included (`#old/child` becomes `#new/child`), inline and in frontmatter
/// `tags`. Case doesn't matter when matching `old`. Code is never touched.
pub fn plan_tag_rename(index: &Index, old: &str, new: &str) -> Result<RefactorPlan, IndexError> {
    let old = old.trim().trim_start_matches('#');
    let new = new.trim().trim_start_matches('#');
    let mut plan = RefactorPlan::default();
    if old.is_empty() || new.is_empty() || old == new {
        return Ok(plan);
    }
    for source in index.sources_tagged(old)? {
        let Some(text) = index.text(&source)? else {
            continue;
        };
        let doc = igneous_markdown::parse(&text);
        let mut edits = Vec::new();
        for tag in &doc.tags {
            if let Some(len) = tag_prefix_len(&tag.name, old) {
                let start = tag.range.start + 1;
                edits.push(TextEdit::replace(start..start + len, new));
            }
        }
        if let Some(fm) = &doc.frontmatter {
            for (name, range) in frontmatter_tags(&text, fm) {
                if let Some(len) = tag_prefix_len(&name, old) {
                    let start = range.end - name.len();
                    edits.push(TextEdit::replace(start..start + len, new));
                }
            }
        }
        if !edits.is_empty() {
            plan.files.push(FileEdits {
                path_after: source.clone(),
                path: source,
                text,
                edits,
            });
        }
    }
    Ok(plan)
}

/// If `name` is the tag `old` or nested under it, the byte length of the
/// part that is `old`.
fn tag_prefix_len(name: &str, old: &str) -> Option<usize> {
    let n = old.chars().count();
    let end = name.char_indices().nth(n).map_or(name.len(), |(i, _)| i);
    let head = &name[..end];
    if head.chars().count() != n || head.to_lowercase() != old.to_lowercase() {
        return None;
    }
    match name[end..].chars().next() {
        None | Some('/') => Some(end),
        _ => None,
    }
}

/// How a link's target is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Wiki,
    /// A Markdown destination, in `<…>` or percent-encoded.
    Markdown {
        angle: bool,
    },
}

/// Where the target part of a link is written (the path, without any
/// heading, block or alias), and how.
fn target_span(text: &str, link: &Link) -> Option<(Span, Syntax)> {
    let whole = link.range.clone();
    let source = text.get(whole.clone())?;
    match link.kind {
        LinkKind::Wiki => {
            let open = if source.starts_with("![[") {
                3
            } else if source.starts_with("[[") {
                2
            } else {
                return None;
            };
            if !source.ends_with("]]") || source.len() < open + 2 {
                return None;
            }
            let inner = &source[open..source.len() - 2];
            let end = inner.find(['#', '|']).unwrap_or(inner.len());
            let raw = &inner[..end];
            let lead = raw.len() - raw.trim_start().len();
            let trimmed = raw.trim().len();
            let start = whole.start + open + lead;
            (trimmed > 0).then_some((start..start + trimmed, Syntax::Wiki))
        }
        LinkKind::Markdown => {
            let label_start = if source.starts_with("![") {
                1
            } else if source.starts_with('[') {
                0
            } else {
                return None;
            };
            let close = matching_bracket(source, label_start)?;
            let rest = source.get(close + 1..)?.strip_prefix('(')?;
            let mut dest_start = close + 2;
            let rest_trimmed = rest.trim_start();
            dest_start += rest.len() - rest_trimmed.len();
            if let Some(inside) = rest_trimmed.strip_prefix('<') {
                let end = inside.find(['>', '\n'])?;
                let dest = &inside[..end];
                let target_len = dest.find('#').unwrap_or(dest.len());
                let start = whole.start + dest_start + 1;
                (target_len > 0)
                    .then_some((start..start + target_len, Syntax::Markdown { angle: true }))
            } else {
                let mut depth = 0usize;
                let mut end = rest_trimmed.len();
                for (i, c) in rest_trimmed.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' if depth == 0 => {
                            end = i;
                            break;
                        }
                        ')' => depth -= 1,
                        c if c.is_whitespace() => {
                            end = i;
                            break;
                        }
                        _ => {}
                    }
                }
                let dest = &rest_trimmed[..end];
                let target_len = dest.find('#').unwrap_or(dest.len());
                let start = whole.start + dest_start;
                (target_len > 0)
                    .then_some((start..start + target_len, Syntax::Markdown { angle: false }))
            }
        }
        _ => None,
    }
}

/// The index of the `]` closing the `[` at `open`, skipping escapes and
/// nested brackets.
fn matching_bracket(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in s[open..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// How to write a link to `to` from `from` in the same style as `target`
/// was written, or `None` if no such link reaches it.
fn new_target_text(
    target: &str,
    method: Method,
    syntax: Syntax,
    to: &VaultPath,
    from: &VaultPath,
    files: &FileSet,
) -> Option<String> {
    let with_md = has_md(target);
    let note = is_note(to);
    let path_text = |p: &str| -> String {
        if note && !with_md {
            strip_md(p).to_owned()
        } else {
            p.to_owned()
        }
    };
    let full = path_text(to.as_str());
    let candidate = match method {
        Method::SameNote => return None,
        Method::Name if target.trim_start_matches('/').contains('/') => full.clone(),
        Method::Name => {
            if files.named(&name_key(to)).len() == 1 {
                path_text(to.file_name())
            } else {
                full.clone()
            }
        }
        Method::Exact if target.starts_with('/') => format!("/{full}"),
        Method::Exact => full.clone(),
        Method::Relative => {
            let rel = path_text(&relative_path(from.parent().as_ref(), to));
            if target.starts_with("./") && !rel.starts_with("../") {
                format!("./{rel}")
            } else {
                rel
            }
        }
    };
    let reaches = |t: &str| files.resolve_target(t, from).map(|r| r.0).as_ref() == Some(to);
    let chosen = if reaches(&candidate) {
        candidate
    } else if reaches(&full) {
        full
    } else {
        return None;
    };
    Some(match syntax {
        Syntax::Wiki | Syntax::Markdown { angle: true } => chosen,
        Syntax::Markdown { angle: false } => percent_encode(&chosen),
    })
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

/// Percent-encodes a Markdown link path the way Obsidian (and
/// igneous-markdown) does.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' | '%' | '(' | ')' | '<' | '>' | '[' | ']' | '^' | '|' | '#' => {
                out.push_str(&format!("%{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_prefixes() {
        assert_eq!(tag_prefix_len("project", "project"), Some(7));
        assert_eq!(tag_prefix_len("Project/igneous", "project"), Some(7));
        assert_eq!(tag_prefix_len("project-x", "project"), None);
        assert_eq!(tag_prefix_len("proj", "project"), None);
        assert_eq!(tag_prefix_len("Écrit/a", "écrit"), Some("Écrit".len()));
    }

    #[test]
    fn target_spans() {
        for (text, expected) in [
            ("[[Note]]", "Note"),
            ("![[ img.png |300]]", "img.png"),
            ("[[a/Note#Head|alias]]", "a/Note"),
            ("[label [x]](../Some%20Note.md#H)", "../Some%20Note.md"),
            ("![alt](<My Note.md>)", "My Note.md"),
            ("[t]( x.md \"title\")", "x.md"),
        ] {
            let doc = igneous_markdown::parse(text);
            let link = doc.links.first().unwrap_or_else(|| panic!("{text}"));
            let (span, _) = target_span(text, link).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(&text[span], expected, "{text}");
        }
    }
}
