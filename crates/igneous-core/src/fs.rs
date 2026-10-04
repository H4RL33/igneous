//! Reading and writing vault files safely.
//!
//! Writes are atomic (temporary file + rename) and guarded: the caller states
//! what it expects to find on disk, and the write is refused if someone else
//! changed the file in the meantime.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::text::{NotUtf8, TextFile};

/// Identifies one version of a file's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    pub hash: blake3::Hash,
    pub mtime: Option<SystemTime>,
}

impl FileStamp {
    pub fn of_bytes(bytes: &[u8], mtime: Option<SystemTime>) -> Self {
        Self {
            size: bytes.len() as u64,
            hash: blake3::hash(bytes),
            mtime,
        }
    }

    /// Same contents, ignoring modification time.
    pub fn same_contents(&self, other: &FileStamp) -> bool {
        self.size == other.size && self.hash == other.hash
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    NotUtf8(#[from] NotUtf8),
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The file on disk is not what the caller expected. `current` is what is
    /// there now (`None` if it no longer exists).
    #[error("file changed on disk")]
    ChangedOnDisk { current: Option<FileStamp> },
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// What the caller expects to find on disk before writing.
#[derive(Debug, Clone, Copy)]
pub enum Expect<'a> {
    /// Write unconditionally.
    Anything,
    /// The file must not exist yet.
    Absent,
    /// The file must still have these contents.
    Contents(&'a FileStamp),
}

pub fn read_text(path: &Path) -> Result<(TextFile, FileStamp), ReadError> {
    let bytes = std::fs::read(path)?;
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let stamp = FileStamp::of_bytes(&bytes, mtime);
    Ok((TextFile::from_bytes(&bytes)?, stamp))
}

/// The stamp of the file as it is now, or `None` if it doesn't exist.
pub fn current_stamp(path: &Path) -> io::Result<Option<FileStamp>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            Ok(Some(FileStamp::of_bytes(&bytes, mtime)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Atomically replaces `path` with `file`, if the precondition holds.
///
/// Symlinks are followed so the link itself is preserved, and the existing
/// file's permissions are kept.
pub fn write_atomic(
    path: &Path,
    file: &TextFile,
    expect: Expect<'_>,
) -> Result<FileStamp, WriteError> {
    write_bytes_atomic(path, &file.to_bytes(), expect)
}

pub fn write_bytes_atomic(
    path: &Path,
    bytes: &[u8],
    expect: Expect<'_>,
) -> Result<FileStamp, WriteError> {
    let target = resolve_symlink(path)?;
    let precondition_holds = |now: &Option<FileStamp>| match expect {
        Expect::Anything => true,
        Expect::Absent => now.is_none(),
        Expect::Contents(expected) => now.as_ref().is_some_and(|now| now.same_contents(expected)),
    };
    if !matches!(expect, Expect::Anything) {
        let now = current_stamp(&target)?;
        if !precondition_holds(&now) {
            return Err(WriteError::ChangedOnDisk { current: now });
        }
    }

    let dir = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let (temp_path, mut temp) = create_temp(dir, &target)?;
    let written = (|| {
        temp.write_all(bytes)?;
        temp.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp_path, meta.permissions())?;
        }
        std::fs::rename(&temp_path, &target)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e.into());
    }

    let mtime = std::fs::metadata(&target).and_then(|m| m.modified()).ok();
    Ok(FileStamp::of_bytes(bytes, mtime))
}

/// A new hidden file next to `target`. Created with the default mode, so a
/// new note gets the same permissions as any other new file.
fn create_temp(dir: &Path, target: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{name}.{}-{n}.igneous-tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn resolve_symlink(path: &Path) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path),
        _ => Ok(path.to_path_buf()),
    }
}

/// Renames `from` to `to`, creating parent folders. Refuses to overwrite.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)
}

/// Moves `path` into `<vault>/.trash/`, keeping its relative location and
/// adding a numeric suffix if something with that name is already there.
/// Returns the new location.
pub fn move_to_vault_trash(vault_root: &Path, path: &Path) -> io::Result<PathBuf> {
    let rel = path
        .strip_prefix(vault_root)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path is outside the vault"))?;
    let mut dest = vault_root.join(".trash").join(rel);
    let mut n = 1;
    while std::fs::symlink_metadata(&dest).is_ok() {
        let stem = rel.file_stem().unwrap_or_default().to_string_lossy();
        let name = match rel.extension() {
            Some(ext) => format!("{stem} {n}.{}", ext.to_string_lossy()),
            None => format!("{stem} {n}"),
        };
        dest.set_file_name(name);
        n += 1;
    }
    rename(path, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_guarded_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");

        let first = write_atomic(&path, &TextFile::new("one\n"), Expect::Absent).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");

        // Writing again with the right expectation succeeds.
        let second =
            write_atomic(&path, &TextFile::new("two\n"), Expect::Contents(&first)).unwrap();

        // Someone else edits the file.
        std::fs::write(&path, "external\n").unwrap();
        let err =
            write_atomic(&path, &TextFile::new("three\n"), Expect::Contents(&second)).unwrap_err();
        match err {
            WriteError::ChangedOnDisk { current: Some(now) } => {
                assert_eq!(now.hash, blake3::hash(b"external\n"))
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
    }

    #[test]
    fn absent_refuses_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "x").unwrap();
        assert!(matches!(
            write_atomic(&path, &TextFile::new("y"), Expect::Absent),
            Err(WriteError::ChangedOnDisk { .. })
        ));
    }

    #[test]
    fn deleted_file_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        let stamp = write_atomic(&path, &TextFile::new("x"), Expect::Anything).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            write_atomic(&path, &TextFile::new("y"), Expect::Contents(&stamp)),
            Err(WriteError::ChangedOnDisk { current: None })
        ));
    }

    #[test]
    fn leaves_no_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        write_atomic(&path, &TextFile::new("x"), Expect::Anything).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["note.md"]);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_get_default_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.md");
        std::fs::write(&plain, "x").unwrap();
        let ours = dir.path().join("ours.md");
        write_atomic(&ours, &TextFile::new("x"), Expect::Absent).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&ours), mode(&plain));
    }

    #[cfg(unix)]
    #[test]
    fn keeps_symlinks_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        std::fs::write(&real, "x").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, &TextFile::new("y"), Expect::Anything).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "y");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn vault_trash_avoids_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("a")).unwrap();
        for _ in 0..2 {
            std::fs::write(root.join("a/n.md"), "x").unwrap();
            move_to_vault_trash(root, &root.join("a/n.md")).unwrap();
        }
        assert!(root.join(".trash/a/n.md").exists());
        assert!(root.join(".trash/a/n 1.md").exists());
    }
}
