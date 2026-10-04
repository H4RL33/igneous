//! An open vault: a root folder plus the rules for what inside it counts.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::ignore::IgnoreRules;
use crate::path::{VaultPath, natural_cmp};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
    ignore: IgnoreRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: VaultPath,
    pub kind: EntryKind,
}

/// A stable identifier for a vault on this machine, derived from its
/// canonical path. Names the vault's cache folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VaultKey(String);

impl VaultKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Vault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = std::fs::canonicalize(root.as_ref())?;
        if !root.is_dir() {
            return Err(VaultError::NotAFolder(root));
        }
        Ok(Self {
            root,
            ignore: IgnoreRules::default(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Display name: the root folder's name.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    /// `<vault>/.igneous/`
    pub fn igneous_dir(&self) -> PathBuf {
        self.root.join(".igneous")
    }

    pub fn key(&self) -> VaultKey {
        use std::os::unix::ffi::OsStrExt;
        let hash = blake3::hash(self.root.as_os_str().as_bytes());
        VaultKey(hash.to_hex()[..32].to_owned())
    }

    pub fn ignore_rules(&self) -> &IgnoreRules {
        &self.ignore
    }

    pub fn set_ignore_rules(&mut self, ignore: IgnoreRules) {
        self.ignore = ignore;
    }

    pub fn is_ignored(&self, path: &VaultPath) -> bool {
        self.ignore.is_ignored(path)
    }

    pub fn abs(&self, path: &VaultPath) -> PathBuf {
        path.to_fs(&self.root)
    }

    pub fn relative(&self, abs: &Path) -> Option<VaultPath> {
        VaultPath::from_fs(&self.root, abs).ok()
    }

    /// The non-ignored entries directly inside `folder` (`None` for the vault
    /// root): folders first, then files, each in natural name order.
    pub fn list(&self, folder: Option<&VaultPath>) -> std::io::Result<Vec<Entry>> {
        let dir = folder.map_or_else(|| self.root.clone(), |f| self.abs(f));
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = match folder {
                Some(f) => f.join(&name),
                None => VaultPath::new(&name),
            };
            let Ok(path) = path else { continue };
            if self.ignore.is_ignored(&path) {
                continue;
            }
            let kind = if entry.file_type()?.is_dir() {
                EntryKind::Folder
            } else {
                EntryKind::File
            };
            entries.push(Entry { path, kind });
        }
        entries.sort_by(|a, b| {
            (a.kind == EntryKind::File)
                .cmp(&(b.kind == EntryKind::File))
                .then_with(|| natural_cmp(a.path.file_name(), b.path.file_name()))
        });
        Ok(entries)
    }

    /// `stem.ext` in `folder`, or `stem 1.ext`, `stem 2.ext`… if taken.
    pub fn unused_name(
        &self,
        folder: Option<&VaultPath>,
        stem: &str,
        extension: &str,
    ) -> VaultPath {
        let name = |n: usize| match (n, extension.is_empty()) {
            (0, true) => stem.to_owned(),
            (0, false) => format!("{stem}.{extension}"),
            (n, true) => format!("{stem} {n}"),
            (n, false) => format!("{stem} {n}.{extension}"),
        };
        (0..)
            .map(|n| match folder {
                Some(f) => f.join(&name(n)),
                None => VaultPath::new(&name(n)),
            })
            .filter_map(Result::ok)
            .find(|p| std::fs::symlink_metadata(self.abs(p)).is_err())
            .expect("some name is free")
    }

    /// Every non-ignored file and folder, depth-first, sorted by name within
    /// each folder. Ignored folders are not descended into. Symlinks are not
    /// followed.
    pub fn walk(&self) -> impl Iterator<Item = Result<Entry, walkdir::Error>> + '_ {
        walkdir::WalkDir::new(&self.root)
            .min_depth(1)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| match self.relative(entry.path()) {
                Some(path) => !self.ignore.is_ignored(&path),
                None => false,
            })
            .filter_map(|entry| match entry {
                Err(e) => Some(Err(e)),
                Ok(entry) => {
                    let path = self.relative(entry.path())?;
                    let kind = if entry.file_type().is_dir() {
                        EntryKind::Folder
                    } else {
                        EntryKind::File
                    };
                    Some(Ok(Entry { path, kind }))
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_skips_ignored_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "Home.md",
            "notes/a.md",
            "notes/.backups/a.md",
            ".obsidian/app.json",
            ".git/HEAD",
            "archive/old.md",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "").unwrap();
        }
        let mut vault = Vault::open(root).unwrap();
        vault.set_ignore_rules(IgnoreRules::new(&["archive"]).unwrap());

        let entries: Vec<String> = vault
            .walk()
            .map(|e| {
                let e = e.unwrap();
                match e.kind {
                    EntryKind::Folder => format!("{}/", e.path),
                    EntryKind::File => e.path.to_string(),
                }
            })
            .collect();
        assert_eq!(entries, ["Home.md", "notes/", "notes/a.md"]);
    }

    #[test]
    fn list_sorts_folders_first_naturally() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "b.md",
            "Note 10.md",
            "Note 2.md",
            "zeta/x.md",
            "Alpha/y.md",
            ".obsidian/a",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "").unwrap();
        }
        let vault = Vault::open(root).unwrap();
        let names: Vec<_> = vault
            .list(None)
            .unwrap()
            .into_iter()
            .map(|e| e.path.to_string())
            .collect();
        assert_eq!(names, ["Alpha", "zeta", "b.md", "Note 2.md", "Note 10.md"]);
        let inner: Vec<_> = vault
            .list(Some(&VaultPath::new("zeta").unwrap()))
            .unwrap()
            .into_iter()
            .map(|e| e.path.to_string())
            .collect();
        assert_eq!(inner, ["zeta/x.md"]);
    }

    #[test]
    fn unused_names() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.unused_name(None, "Untitled", "md").as_str(),
            "Untitled.md"
        );
        std::fs::write(dir.path().join("Untitled.md"), "").unwrap();
        std::fs::write(dir.path().join("Untitled 1.md"), "").unwrap();
        assert_eq!(
            vault.unused_name(None, "Untitled", "md").as_str(),
            "Untitled 2.md"
        );
        std::fs::create_dir(dir.path().join("Untitled")).unwrap();
        assert_eq!(
            vault.unused_name(None, "Untitled", "").as_str(),
            "Untitled 1"
        );
    }

    #[test]
    fn key_is_stable_and_path_based() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let va = Vault::open(a.path()).unwrap();
        assert_eq!(va.key(), Vault::open(a.path()).unwrap().key());
        assert_ne!(va.key(), Vault::open(b.path()).unwrap().key());
        assert_eq!(va.key().as_str().len(), 32);
    }

    #[test]
    fn rejects_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.md");
        std::fs::write(&file, "").unwrap();
        assert!(matches!(Vault::open(&file), Err(VaultError::NotAFolder(_))));
    }
}
