//! Running `git`.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use igneous_core::settings::SyncMethod;

use crate::status::{self, InProgress, RepoStatus};
use crate::{FailureKind, GitError};

/// The oldest Git Igneous supports (for `git restore` and porcelain v2).
pub const MIN_VERSION: (u32, u32) = (2, 30);

/// Commands that only touch the local repository.
const LOCAL: Duration = Duration::from_secs(60);
/// Commands that talk to a remote.
const NETWORK: Duration = Duration::from_secs(180);
/// How long to keep retrying while another program holds `index.lock`.
const LOCK_BACKOFF: [u64; 5] = [100, 250, 500, 1000, 2000];

/// A repository and the `git` command to run in it.
///
/// Paths passed in and out are relative to the repository's top level, which
/// may be a parent of the vault. [`Git::to_vault`] and [`Git::to_repo`]
/// convert between the two.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
    top: PathBuf,
    git_dir: PathBuf,
    /// The vault's folder relative to `top`, ending in `/`, or empty.
    prefix: String,
    version: (u32, u32),
    envs: Vec<(OsString, OsString)>,
}

/// One commit in a file's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
    /// The file's path (relative to the repository) in this commit.
    pub path: String,
}

struct Exit {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}

impl Git {
    /// Finds the repository containing `dir`. Returns `None` if there isn't
    /// one, and an error if `git` is missing or too old.
    pub fn discover(dir: &Path) -> Result<Option<Git>, GitError> {
        Self::discover_with(dir, Vec::new())
    }

    /// Like [`Git::discover`], with extra environment variables for every
    /// command (tests use this to shut out the user's Git configuration).
    pub fn discover_with(
        dir: &Path,
        envs: Vec<(OsString, OsString)>,
    ) -> Result<Option<Git>, GitError> {
        let program = find_program("git").ok_or(GitError::NotInstalled)?;
        let mut git = Git {
            program,
            top: dir.to_path_buf(),
            git_dir: PathBuf::new(),
            prefix: String::new(),
            version: (0, 0),
            envs,
        };
        let out = git.exec(&["--version"], LOCAL)?;
        let text = String::from_utf8_lossy(&out.stdout);
        git.version = parse_version(&text).ok_or_else(|| GitError::Parse(text.to_string()))?;
        if git.version < MIN_VERSION {
            return Err(GitError::TooOld {
                found: text.trim().trim_start_matches("git version ").to_owned(),
            });
        }
        let out = git.exec(
            &["rev-parse", "--show-toplevel", "--absolute-git-dir"],
            LOCAL,
        )?;
        if !out.success {
            if out.stderr.contains("not a git repository") {
                return Ok(None);
            }
            return Err(GitError::from_output("", &out.stderr));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else {
            return Err(GitError::Parse(text.to_string()));
        };
        git.top = PathBuf::from(top);
        git.git_dir = PathBuf::from(git_dir);
        let dir = dir.canonicalize().map_err(GitError::Spawn)?;
        let top = git.top.canonicalize().map_err(GitError::Spawn)?;
        if let Ok(rel) = dir.strip_prefix(&top)
            && !rel.as_os_str().is_empty()
        {
            git.prefix = format!("{}/", rel.to_string_lossy().replace('\\', "/"));
        }
        Ok(Some(git))
    }

    /// The repository's top-level folder.
    pub fn top_level(&self) -> &Path {
        &self.top
    }

    pub fn version(&self) -> (u32, u32) {
        self.version
    }

    /// The vault-relative form of a repository path, or `None` if the path
    /// is outside the vault.
    pub fn to_vault<'a>(&self, repo_path: &'a str) -> Option<&'a str> {
        repo_path.strip_prefix(self.prefix.as_str())
    }

    /// The repository-relative form of a vault path.
    pub fn to_repo(&self, vault_path: &str) -> String {
        format!("{}{vault_path}", self.prefix)
    }

    /// The vault's folder as a pathspec.
    fn vault_spec(&self) -> &str {
        if self.prefix.is_empty() {
            "."
        } else {
            &self.prefix
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(&self.top)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Never wait on a prompt nobody can see.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            // Messages are matched in English.
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            // Merges and rebases continue without opening an editor.
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            // Paths are paths, never globs.
            .env("GIT_LITERAL_PATHSPECS", "1");
        for (key, value) in &self.envs {
            cmd.env(key, value);
        }
        cmd
    }

    fn exec(&self, args: &[&str], timeout: Duration) -> Result<Exit, GitError> {
        self.exec_cmd(self.command(args), timeout)
    }

    fn exec_cmd(&self, mut cmd: Command, timeout: Duration) -> Result<Exit, GitError> {
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => GitError::NotInstalled,
            _ => GitError::Spawn(e),
        })?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let out = thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        let err = thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(GitError::Spawn)? {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::Timeout(timeout));
            }
            thread::sleep(Duration::from_millis(5));
        };
        Ok(Exit {
            success: status.success(),
            stdout: out.join().unwrap_or_default(),
            stderr: String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned(),
        })
    }

    /// Runs `git args…`, retrying while another program holds the index lock.
    /// Fails if git exits unsuccessfully.
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>, GitError> {
        let mut backoff = LOCK_BACKOFF.iter();
        loop {
            tracing::debug!(?args, "git");
            let exit = self.exec(args, timeout)?;
            if exit.success {
                return Ok(exit.stdout);
            }
            let error = GitError::from_output(&String::from_utf8_lossy(&exit.stdout), &exit.stderr);
            match (error.kind(), backoff.next()) {
                (FailureKind::Locked, Some(ms)) => thread::sleep(Duration::from_millis(*ms)),
                _ => return Err(error),
            }
        }
    }

    // --- status ----------------------------------------------------------

    /// The branch and every changed path in the vault.
    pub fn status(&self) -> Result<RepoStatus, GitError> {
        let mut cmd = self.command(&[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--",
            self.vault_spec(),
        ]);
        // Polling status shouldn't take locks other programs might want.
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        let exit = self.exec_cmd(cmd, LOCAL)?;
        if !exit.success {
            return Err(GitError::from_output("", &exit.stderr));
        }
        let mut status = status::parse(&exit.stdout)?;
        status.in_progress = self.in_progress();
        Ok(status)
    }

    fn in_progress(&self) -> Option<InProgress> {
        let has = |name: &str| self.git_dir.join(name).exists();
        if has("rebase-merge") || has("rebase-apply") {
            Some(InProgress::Rebase)
        } else if has("MERGE_HEAD") {
            Some(InProgress::Merge)
        } else if has("CHERRY_PICK_HEAD") {
            Some(InProgress::CherryPick)
        } else if has("REVERT_HEAD") {
            Some(InProgress::Revert)
        } else {
            None
        }
    }

    pub fn remotes(&self) -> Result<Vec<String>, GitError> {
        let out = self.run(&["remote"], LOCAL)?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    // --- staging -----------------------------------------------------------

    pub fn stage(&self, paths: &[&str]) -> Result<(), GitError> {
        self.with_paths(&["add", "-A"], paths).map(drop)
    }

    /// Stages every change in the vault.
    pub fn stage_all(&self) -> Result<(), GitError> {
        self.run(&["add", "-A", "--", self.vault_spec()], LOCAL)
            .map(drop)
    }

    pub fn unstage(&self, paths: &[&str]) -> Result<(), GitError> {
        if self.has_commits()? {
            self.with_paths(&["restore", "--staged"], paths).map(drop)
        } else {
            self.with_paths(&["rm", "--cached", "-r", "-q"], paths)
                .map(drop)
        }
    }

    /// Throws away unstaged changes to tracked files, restoring them as they
    /// are staged (or committed). Untracked files are left alone; deleting
    /// them is up to the caller.
    pub fn discard(&self, paths: &[&str]) -> Result<(), GitError> {
        self.with_paths(&["restore", "--worktree"], paths).map(drop)
    }

    fn with_paths(&self, args: &[&str], paths: &[&str]) -> Result<Vec<u8>, GitError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut all: Vec<&str> = args.to_vec();
        all.push("--");
        all.extend_from_slice(paths);
        self.run(&all, LOCAL)
    }

    fn has_commits(&self) -> Result<bool, GitError> {
        let exit = self.exec(&["rev-parse", "--verify", "-q", "HEAD"], LOCAL)?;
        Ok(exit.success)
    }

    pub fn head(&self) -> Result<Option<String>, GitError> {
        let exit = self.exec(&["rev-parse", "--verify", "-q", "HEAD"], LOCAL)?;
        Ok(exit
            .success
            .then(|| String::from_utf8_lossy(&exit.stdout).trim().to_owned()))
    }

    // --- committing ----------------------------------------------------------

    /// Commits what's staged. Returns `false` if nothing was staged.
    pub fn commit(&self, message: &str) -> Result<bool, GitError> {
        let staged = self.exec(&["diff", "--cached", "--quiet"], LOCAL)?;
        if staged.success {
            return Ok(false);
        }
        self.run(&["commit", "-q", "-m", message], LOCAL)?;
        Ok(true)
    }

    /// Finishes a merge or rebase once its conflicts are resolved and staged.
    pub fn conclude(&self) -> Result<(), GitError> {
        match self.in_progress() {
            Some(InProgress::Merge) => self.run(&["commit", "-q", "--no-edit"], LOCAL).map(drop),
            Some(InProgress::Rebase) => self.run(&["rebase", "--continue"], LOCAL).map(drop),
            Some(InProgress::CherryPick) => {
                self.run(&["cherry-pick", "--continue"], LOCAL).map(drop)
            }
            Some(InProgress::Revert) => self.run(&["revert", "--continue"], LOCAL).map(drop),
            None => Ok(()),
        }
    }

    // --- remotes -------------------------------------------------------------

    pub fn fetch(&self) -> Result<(), GitError> {
        self.run(&["fetch", "-q"], NETWORK).map(drop)
    }

    pub fn pull(&self, method: SyncMethod) -> Result<(), GitError> {
        let args: &[&str] = match method {
            // `--ff` overrides a `pull.ff = only` setting, which would
            // otherwise refuse every divergent pull.
            SyncMethod::Merge => &["pull", "-q", "--no-rebase", "--ff", "--no-edit"],
            SyncMethod::Rebase => &["pull", "-q", "--rebase"],
        };
        self.run(args, NETWORK).map(drop)
    }

    /// Moves the branch up to its upstream if that needs no merge. Refuses
    /// (safely) if local changes would be overwritten.
    pub fn fast_forward(&self) -> Result<(), GitError> {
        self.run(&["merge", "-q", "--ff-only", "@{u}"], LOCAL)
            .map(drop)
    }

    pub fn push(&self) -> Result<(), GitError> {
        self.run(&["push", "-q"], NETWORK).map(drop)
    }

    /// Pushes the branch to `remote` and makes that its upstream.
    pub fn publish(&self, remote: &str) -> Result<(), GitError> {
        self.run(&["push", "-q", "-u", remote, "HEAD"], NETWORK)
            .map(drop)
    }

    // --- history -------------------------------------------------------------

    /// The diff for one path: staged changes if `staged`, otherwise the
    /// unstaged ones (an untracked file shows as entirely added).
    pub fn diff(&self, path: &str, staged: bool, untracked: bool) -> Result<String, GitError> {
        let exit = if untracked {
            self.exec(
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-index",
                    "--",
                    "/dev/null",
                    path,
                ],
                LOCAL,
            )?
        } else {
            let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
            if staged {
                args.push("--cached");
            }
            args.extend(["--", path]);
            self.exec(&args, LOCAL)?
        };
        // `--no-index` exits with 1 when the files differ.
        if !exit.success && !(untracked && exit.stderr.is_empty()) {
            return Err(GitError::from_output("", &exit.stderr));
        }
        Ok(String::from_utf8_lossy(&exit.stdout).into_owned())
    }

    /// Commits that touched `path`, newest first, following renames.
    pub fn log(&self, path: &str, limit: usize) -> Result<Vec<Commit>, GitError> {
        let limit = limit.to_string();
        let out = self.run(
            &[
                "log",
                "--follow",
                "--name-only",
                "-n",
                &limit,
                "--format=%x1e%H%x1f%h%x1f%an%x1f%at%x1f%s",
                "--",
                path,
            ],
            LOCAL,
        )?;
        let text = String::from_utf8_lossy(&out);
        text.split('\x1e')
            .filter(|r| !r.trim().is_empty())
            .map(|record| {
                let mut lines = record.lines();
                let header = lines.next().unwrap_or_default();
                let fields: Vec<&str> = header.split('\x1f').collect();
                let [hash, short, author, time, subject] = fields[..] else {
                    return Err(GitError::Parse(record.to_owned()));
                };
                let name = lines.map(str::trim).find(|l| !l.is_empty());
                Ok(Commit {
                    hash: hash.to_owned(),
                    short: short.to_owned(),
                    author: author.to_owned(),
                    time: time.parse().unwrap_or(0),
                    subject: subject.to_owned(),
                    path: name.unwrap_or(path).to_owned(),
                })
            })
            .collect()
    }

    /// The contents of `path` as of `rev`. Use the path the file had in that
    /// commit ([`Commit::path`]).
    pub fn show(&self, rev: &str, path: &str) -> Result<Vec<u8>, GitError> {
        self.run(&["show", &format!("{rev}:{path}")], LOCAL)
    }

    // --- git-crypt -------------------------------------------------------------

    /// Whether `.gitattributes` sends files through git-crypt.
    pub fn uses_git_crypt(&self) -> bool {
        [
            self.top.join(".gitattributes"),
            self.git_dir.join("info").join("attributes"),
        ]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .any(|text| text.contains("filter=git-crypt"))
    }

    /// Refuses to go on if the repository uses git-crypt but it isn't
    /// installed or unlocked here: committing then could push plaintext, or
    /// fail half-way. Full git-crypt support is milestone M9.
    pub fn check_git_crypt(&self) -> Result<(), GitError> {
        if !self.uses_git_crypt() {
            return Ok(());
        }
        if find_program("git-crypt").is_none() {
            return Err(GitError::failed(
                FailureKind::GitCrypt,
                "This vault uses git-crypt, which isn't installed",
            ));
        }
        let exit = self.exec(&["config", "--get", "filter.git-crypt.clean"], LOCAL)?;
        if !exit.success || exit.stdout.trim_ascii().is_empty() {
            return Err(GitError::failed(
                FailureKind::GitCrypt,
                "This vault uses git-crypt but isn't unlocked",
            ));
        }
        Ok(())
    }
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let version = text.trim().strip_prefix("git version ")?;
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Finds `name` on `PATH`.
pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("git version 2.51.0\n"), Some((2, 51)));
        assert_eq!(
            parse_version("git version 2.39.5 (Apple Git-154)"),
            Some((2, 39))
        );
        assert_eq!(parse_version("nope"), None);
    }
}
