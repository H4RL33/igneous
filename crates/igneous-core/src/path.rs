//! Vault-relative paths.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// A path relative to the vault root, always `/`-separated, never empty, and
/// never escaping the vault.
///
/// The on-disk spelling is preserved exactly. Use [`VaultPath::loose_key`] for
/// the case-insensitive, Unicode-normalised comparisons that link resolution
/// needs.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VaultPath(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path must be relative to the vault: {0}")]
    NotRelative(String),
    #[error("path leaves the vault: {0}")]
    Escapes(String),
    #[error("path is not inside the vault: {0}")]
    OutsideVault(String),
    #[error("path is not valid UTF-8")]
    NotUtf8,
}

impl VaultPath {
    /// Parses a `/`-separated relative path, dropping empty and `.` components.
    pub fn new(path: &str) -> Result<Self, PathError> {
        if path.starts_with('/') {
            return Err(PathError::NotRelative(path.to_owned()));
        }
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(PathError::Escapes(path.to_owned())),
                part => parts.push(part),
            }
        }
        if parts.is_empty() {
            return Err(PathError::Empty);
        }
        Ok(Self(parts.join("/")))
    }

    /// Converts an absolute filesystem path inside `root` to a vault path.
    pub fn from_fs(root: &Path, path: &Path) -> Result<Self, PathError> {
        let rel = path
            .strip_prefix(root)
            .map_err(|_| PathError::OutsideVault(path.display().to_string()))?;
        let mut parts = Vec::new();
        for component in rel.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_str().ok_or(PathError::NotUtf8)?),
                Component::CurDir => {}
                _ => return Err(PathError::Escapes(path.display().to_string())),
            }
        }
        if parts.is_empty() {
            return Err(PathError::Empty);
        }
        Ok(Self(parts.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_fs(&self, root: &Path) -> PathBuf {
        let mut path = root.to_path_buf();
        path.extend(self.components());
        path
    }

    pub fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.0.split('/')
    }

    /// The last component, including any extension.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The file name without its extension. A leading dot is not an extension.
    pub fn stem(&self) -> &str {
        let name = self.file_name();
        match name.rfind('.') {
            Some(0) | None => name,
            Some(dot) => &name[..dot],
        }
    }

    pub fn extension(&self) -> Option<&str> {
        let name = self.file_name();
        match name.rfind('.') {
            Some(0) | None => None,
            Some(dot) => Some(&name[dot + 1..]),
        }
    }

    pub fn parent(&self) -> Option<VaultPath> {
        self.0
            .rfind('/')
            .map(|slash| Self(self.0[..slash].to_owned()))
    }

    pub fn join(&self, child: &str) -> Result<VaultPath, PathError> {
        Self::new(&format!("{}/{child}", self.0))
    }

    pub fn with_extension(&self, extension: &str) -> VaultPath {
        let stem_end = self.0.len() - self.file_name().len() + self.stem().len();
        if extension.is_empty() {
            Self(self.0[..stem_end].to_owned())
        } else {
            Self(format!("{}.{extension}", &self.0[..stem_end]))
        }
    }

    pub fn starts_with(&self, prefix: &VaultPath) -> bool {
        self.0 == prefix.0
            || (self.0.starts_with(&prefix.0) && self.0.as_bytes()[prefix.0.len()] == b'/')
    }

    /// Case-folded, NFC-normalised form used for loose comparisons.
    pub fn loose_key(&self) -> String {
        loose_key(&self.0)
    }

    pub fn eq_loose(&self, other: &VaultPath) -> bool {
        self.0 == other.0 || self.loose_key() == other.loose_key()
    }
}

/// Orders names the way people expect: case-insensitively, with runs of
/// digits compared by value ("Note 2" before "Note 10").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let ord = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                let ord = c.to_lowercase().cmp(d.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Case-folds and NFC-normalises arbitrary text the same way as
/// [`VaultPath::loose_key`].
pub fn loose_key(text: &str) -> String {
    text.nfc().flat_map(char::to_lowercase).collect()
}

impl fmt::Display for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VaultPath({:?})", self.0)
    }
}

impl serde::Serialize for VaultPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for VaultPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        VaultPath::new(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_separators() {
        assert_eq!(VaultPath::new("a//b/./c.md").unwrap().as_str(), "a/b/c.md");
        assert_eq!(VaultPath::new("a/b/").unwrap().as_str(), "a/b");
    }

    #[test]
    fn rejects_bad_paths() {
        assert_eq!(VaultPath::new(""), Err(PathError::Empty));
        assert_eq!(VaultPath::new("./"), Err(PathError::Empty));
        assert!(matches!(
            VaultPath::new("/a"),
            Err(PathError::NotRelative(_))
        ));
        assert!(matches!(
            VaultPath::new("a/../b"),
            Err(PathError::Escapes(_))
        ));
    }

    #[test]
    fn name_parts() {
        let p = VaultPath::new("notes/Daily 2026.10.04.md").unwrap();
        assert_eq!(p.file_name(), "Daily 2026.10.04.md");
        assert_eq!(p.stem(), "Daily 2026.10.04");
        assert_eq!(p.extension(), Some("md"));
        assert_eq!(p.parent().unwrap().as_str(), "notes");
        assert_eq!(
            p.with_extension("base").as_str(),
            "notes/Daily 2026.10.04.base"
        );

        let dot = VaultPath::new(".gitignore").unwrap();
        assert_eq!(dot.stem(), ".gitignore");
        assert_eq!(dot.extension(), None);
        assert!(VaultPath::new("top.md").unwrap().parent().is_none());
    }

    #[test]
    fn prefix_respects_components() {
        let dir = VaultPath::new("notes").unwrap();
        assert!(VaultPath::new("notes/a.md").unwrap().starts_with(&dir));
        assert!(VaultPath::new("notes").unwrap().starts_with(&dir));
        assert!(!VaultPath::new("notes2/a.md").unwrap().starts_with(&dir));
    }

    #[test]
    fn loose_comparison() {
        // "é" precomposed vs decomposed, and case differences.
        let a = VaultPath::new("Caf\u{e9}/Note.md").unwrap();
        let b = VaultPath::new("cafe\u{301}/note.MD").unwrap();
        assert_ne!(a, b);
        assert!(a.eq_loose(&b));
    }

    #[test]
    fn natural_order() {
        let mut names = vec![
            "Note 10", "note 2", "Note 1", "apple", "Banana", "Note 02b", "Note 2a",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "apple", "Banana", "Note 1", "note 2", "Note 2a", "Note 02b", "Note 10"
            ]
        );
    }

    #[test]
    fn fs_round_trip() {
        let root = Path::new("/vault");
        let p = VaultPath::new("a/b.md").unwrap();
        assert_eq!(p.to_fs(root), Path::new("/vault/a/b.md"));
        assert_eq!(VaultPath::from_fs(root, &p.to_fs(root)).unwrap(), p);
        assert!(VaultPath::from_fs(root, Path::new("/elsewhere/x.md")).is_err());
    }
}
