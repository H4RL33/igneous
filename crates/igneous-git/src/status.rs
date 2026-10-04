//! `git status --porcelain=v2 --branch -z`, parsed.

use crate::GitError;

/// A merge, rebase or similar that stopped part-way, usually for conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub entries: Vec<Entry>,
    pub in_progress: Option<InProgress>,
}

impl RepoStatus {
    pub fn conflicts(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.conflicted)
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|e| e.conflicted)
    }

    pub fn staged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_staged())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_unstaged())
    }

    /// Whether there's anything to commit, staged or not.
    pub fn is_dirty(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// One changed path. Paths are relative to the repository's top level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The old path of a rename or copy.
    pub orig_path: Option<String>,
    /// The index (staged) status: `.` for unchanged, otherwise one of git's
    /// letters (`M`, `A`, `D`, `R`, `C`, `T`, `U`).
    pub index: char,
    /// The work tree (unstaged) status, likewise. `?` for untracked files.
    pub worktree: char,
    pub conflicted: bool,
}

impl Entry {
    pub fn is_untracked(&self) -> bool {
        self.worktree == '?'
    }

    pub fn is_staged(&self) -> bool {
        !self.conflicted && self.index != '.' && !self.is_untracked()
    }

    pub fn is_unstaged(&self) -> bool {
        !self.conflicted && self.worktree != '.'
    }

    /// One letter summarising the change, preferring the work tree's view.
    pub fn letter(&self) -> char {
        if self.conflicted {
            'U'
        } else if self.is_untracked() {
            'A'
        } else if self.worktree != '.' {
            self.worktree
        } else {
            self.index
        }
    }
}

/// Parses `git status --porcelain=v2 --branch -z` output.
pub fn parse(output: &[u8]) -> Result<RepoStatus, GitError> {
    let text = String::from_utf8_lossy(output);
    let mut status = RepoStatus::default();
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    let bad = |record: &str| GitError::Parse(format!("status line “{record}”"));
    while let Some(record) = records.next() {
        let (tag, rest) = record.split_once(' ').ok_or_else(|| bad(record))?;
        match tag {
            "#" => header(&mut status, rest),
            "1" => {
                // XY sub mH mI mW hH hI path
                let fields: Vec<&str> = rest.splitn(8, ' ').collect();
                let [xy, _, _, _, _, _, _, path] = fields[..] else {
                    return Err(bad(record));
                };
                status
                    .entries
                    .push(ordinary(xy, path, None).ok_or_else(|| bad(record))?);
            }
            "2" => {
                // XY sub mH mI mW hH hI Xscore path, then the original path.
                let fields: Vec<&str> = rest.splitn(9, ' ').collect();
                let [xy, _, _, _, _, _, _, _, path] = fields[..] else {
                    return Err(bad(record));
                };
                let orig = records.next().ok_or_else(|| bad(record))?;
                status
                    .entries
                    .push(ordinary(xy, path, Some(orig)).ok_or_else(|| bad(record))?);
            }
            "u" => {
                // XY sub m1 m2 m3 mW h1 h2 h3 path
                let fields: Vec<&str> = rest.splitn(10, ' ').collect();
                let [xy, _, _, _, _, _, _, _, _, path] = fields[..] else {
                    return Err(bad(record));
                };
                let mut entry = ordinary(xy, path, None).ok_or_else(|| bad(record))?;
                entry.conflicted = true;
                status.entries.push(entry);
            }
            "?" => status.entries.push(Entry {
                path: rest.to_owned(),
                orig_path: None,
                index: '.',
                worktree: '?',
                conflicted: false,
            }),
            "!" => {}
            _ => return Err(bad(record)),
        }
    }
    Ok(status)
}

fn ordinary(xy: &str, path: &str, orig: Option<&str>) -> Option<Entry> {
    let mut chars = xy.chars();
    let (index, worktree) = (chars.next()?, chars.next()?);
    Some(Entry {
        path: path.to_owned(),
        orig_path: orig.map(str::to_owned),
        index,
        worktree,
        conflicted: false,
    })
}

fn header(status: &mut RepoStatus, line: &str) {
    let (key, value) = line.split_once(' ').unwrap_or((line, ""));
    match key {
        "branch.oid" if value != "(initial)" => status.head = Some(value.to_owned()),
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_owned()),
        "branch.upstream" => status.upstream = Some(value.to_owned()),
        "branch.ab" => {
            for part in value.split(' ') {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "1111111111111111111111111111111111111111";

    fn records(lines: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            out.extend_from_slice(line.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn branch_headers() {
        let out = records(&[
            &format!("# branch.oid {OID}"),
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
        ]);
        let status = parse(&out).unwrap();
        assert_eq!(status.head.as_deref(), Some(OID));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        assert!(!status.is_dirty());
    }

    #[test]
    fn new_repository_and_detached_head() {
        let status = parse(&records(&["# branch.oid (initial)", "# branch.head main"])).unwrap();
        assert_eq!(status.head, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
        let status = parse(&records(&[
            &format!("# branch.oid {OID}"),
            "# branch.head (detached)",
        ]))
        .unwrap();
        assert_eq!(status.branch, None);
    }

    #[test]
    fn entries_of_every_kind() {
        let out = records(&[
            &format!("1 .M N... 100644 100644 100644 {OID} {OID} Daily/2026-10-04.md"),
            &format!("1 A. N... 000000 100644 100644 {OID} {OID} New note.md"),
            &format!("2 R. N... 100644 100644 100644 {OID} {OID} R100 Projects/Moved.md"),
            "Old name.md",
            &format!("u UU N... 100644 100644 100644 100644 {OID} {OID} {OID} Home.md"),
            "? Untracked with spaces.md",
            "! ignored.tmp",
        ]);
        let status = parse(&out).unwrap();
        let e = &status.entries;
        assert_eq!(e.len(), 5);
        assert_eq!(e[0].path, "Daily/2026-10-04.md");
        assert!(e[0].is_unstaged() && !e[0].is_staged());
        assert_eq!(e[0].letter(), 'M');
        assert_eq!(e[1].path, "New note.md");
        assert!(e[1].is_staged() && !e[1].is_unstaged());
        assert_eq!(e[2].path, "Projects/Moved.md");
        assert_eq!(e[2].orig_path.as_deref(), Some("Old name.md"));
        assert_eq!(e[2].letter(), 'R');
        assert!(e[3].conflicted && !e[3].is_staged() && !e[3].is_unstaged());
        assert_eq!(e[4].path, "Untracked with spaces.md");
        assert!(e[4].is_untracked() && e[4].is_unstaged() && !e[4].is_staged());
        assert_eq!(status.conflicts().count(), 1);
        assert_eq!(status.staged().count(), 2);
        assert_eq!(status.unstaged().count(), 2);
    }

    #[test]
    fn malformed_lines_are_errors() {
        assert!(parse(b"1 .M\0").is_err());
        assert!(parse(b"Z what\0").is_err());
        assert!(parse(format!("2 R. N... 1 1 1 {OID} {OID} R100 a.md\0").as_bytes()).is_err());
    }
}
