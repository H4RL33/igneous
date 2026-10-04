//! Link targets: `[[Note#Heading|alias]]` and `[text](Note.md#Heading)`.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subpath {
    /// `#Heading#Subheading`
    Heading(Vec<String>),
    /// `#^block-id`
    Block(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkRef {
    /// The note or file name/path, as written. Empty for same-note links
    /// such as `[[#Heading]]`.
    pub target: String,
    pub subpath: Option<Subpath>,
    /// Alias (`[[target|alias]]`) or Markdown link text.
    pub display: Option<String>,
    /// Embed size: `![[image.png|300]]` or `|300x200`.
    pub size: Option<(u32, Option<u32>)>,
}

impl LinkRef {
    /// Parses the inside of a wikilink, e.g. `Note#Heading|alias`.
    pub fn parse_wiki(inner: &str, embed: bool) -> Self {
        let (path, display) = match inner.split_once('|') {
            Some((path, display)) => (path, Some(display.trim())),
            None => (inner, None),
        };
        let mut link = Self::parse_path(path.trim());
        match display {
            Some(display) if embed && parse_size(display).is_some() => {
                link.size = parse_size(display);
            }
            Some(display) if !display.is_empty() => link.display = Some(display.to_owned()),
            _ => {}
        }
        link
    }

    /// Parses a Markdown link destination, e.g. `Note%20name.md#Heading` or
    /// `<Note name.md>`.
    pub fn parse_markdown(dest: &str) -> Self {
        let dest = dest
            .strip_prefix('<')
            .and_then(|d| d.strip_suffix('>'))
            .unwrap_or(dest);
        if is_external(dest) {
            return Self {
                target: dest.to_owned(),
                ..Self::default()
            };
        }
        Self::parse_path(&percent_decode(dest))
    }

    fn parse_path(path: &str) -> Self {
        let (target, subpath) = match path.split_once('#') {
            None => (path, None),
            Some((target, rest)) => {
                let subpath = match rest.strip_prefix('^') {
                    Some(block) => Subpath::Block(block.trim().to_owned()),
                    None => Subpath::Heading(
                        rest.split('#')
                            .map(str::trim)
                            .filter(|h| !h.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    ),
                };
                (target, Some(subpath))
            }
        };
        Self {
            target: target.trim().to_owned(),
            subpath,
            ..Self::default()
        }
    }

    /// True for URLs (`https://…`, `mailto:…`, `obsidian://…`).
    pub fn is_external(&self) -> bool {
        is_external(&self.target)
    }

    fn subpath_suffix(&self) -> String {
        match &self.subpath {
            None => String::new(),
            Some(Subpath::Block(id)) => format!("#^{id}"),
            Some(Subpath::Heading(path)) => path.iter().fold(String::new(), |mut s, h| {
                let _ = write!(s, "#{h}");
                s
            }),
        }
    }

    /// Formats as a wikilink (without the `[[`, `]]`).
    pub fn to_wiki_inner(&self) -> String {
        let mut out = format!("{}{}", self.target, self.subpath_suffix());
        if let Some((w, h)) = self.size {
            let _ = write!(out, "|{w}");
            if let Some(h) = h {
                let _ = write!(out, "x{h}");
            }
        } else if let Some(display) = &self.display {
            let _ = write!(out, "|{display}");
        }
        out
    }

    /// Formats as a Markdown link destination, percent-encoding as Obsidian does.
    pub fn to_markdown_dest(&self) -> String {
        if self.is_external() {
            return self.target.clone();
        }
        let mut out = percent_encode(&self.target);
        out.push_str(&percent_encode_subpath(&self.subpath_suffix()));
        out
    }
}

fn parse_size(s: &str) -> Option<(u32, Option<u32>)> {
    match s.split_once('x') {
        Some((w, h)) => Some((w.parse().ok()?, Some(h.parse().ok()?))),
        None => Some((s.parse().ok()?, None)),
    }
}

fn is_external(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s[..colon];
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' | '%' | '(' | ')' | '<' | '>' | '[' | ']' | '^' | '|' | '#' => {
                let _ = write!(out, "%{:02X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn percent_encode_subpath(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if c == ' ' {
            out.push_str("%20");
        } else {
            out.push(c);
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_forms() {
        let l = LinkRef::parse_wiki("Note", false);
        assert_eq!(
            (l.target.as_str(), &l.subpath, &l.display),
            ("Note", &None, &None)
        );

        let l = LinkRef::parse_wiki("folder/Note#Heading#Sub|Shown text", false);
        assert_eq!(l.target, "folder/Note");
        assert_eq!(
            l.subpath,
            Some(Subpath::Heading(vec!["Heading".into(), "Sub".into()]))
        );
        assert_eq!(l.display.as_deref(), Some("Shown text"));

        let l = LinkRef::parse_wiki("Note#^abc-123", false);
        assert_eq!(l.subpath, Some(Subpath::Block("abc-123".into())));

        let l = LinkRef::parse_wiki("#Local heading", false);
        assert_eq!(l.target, "");
        assert_eq!(
            l.subpath,
            Some(Subpath::Heading(vec!["Local heading".into()]))
        );
    }

    #[test]
    fn embed_sizes() {
        let l = LinkRef::parse_wiki("image.png|300", true);
        assert_eq!(l.size, Some((300, None)));
        assert_eq!(l.display, None);
        let l = LinkRef::parse_wiki("image.png|300x200", true);
        assert_eq!(l.size, Some((300, Some(200))));
        // Not an embed: a numeric alias is just an alias.
        let l = LinkRef::parse_wiki("Note|300", false);
        assert_eq!(l.display.as_deref(), Some("300"));
        assert_eq!(l.to_wiki_inner(), "Note|300");
    }

    #[test]
    fn markdown_destinations() {
        let l = LinkRef::parse_markdown("My%20Note.md#Some%20Heading");
        assert_eq!(l.target, "My Note.md");
        assert_eq!(
            l.subpath,
            Some(Subpath::Heading(vec!["Some Heading".into()]))
        );
        assert_eq!(l.to_markdown_dest(), "My%20Note.md#Some%20Heading");

        let l = LinkRef::parse_markdown("<Note with spaces.md>");
        assert_eq!(l.target, "Note with spaces.md");

        let l = LinkRef::parse_markdown("https://example.com/a%20b#frag");
        assert!(l.is_external());
        assert_eq!(l.target, "https://example.com/a%20b#frag");
        assert!(LinkRef::parse_markdown("mailto:x@y.z").is_external());
        assert!(!LinkRef::parse_markdown("notes/a.md").is_external());
    }

    #[test]
    fn wiki_round_trip() {
        for inner in ["Note", "a/b#H1#H2|alias", "Note#^id", "img.png|300x200"] {
            let embed = inner.starts_with("img");
            assert_eq!(LinkRef::parse_wiki(inner, embed).to_wiki_inner(), inner);
        }
    }
}
