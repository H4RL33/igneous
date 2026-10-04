//! Commit-and-sync and pull-only passes.
//!
//! These run on a worker thread. Scheduling (intervals, pull on open, "Sync
//! now"), saving open notes first, and pausing after a conflict are the app's
//! job; this module does one pass and reports what happened.
//!
//! A pass never resolves a conflict, never force-pushes, and never runs while
//! a merge or rebase is unfinished.

use igneous_core::settings::SyncMethod;

use crate::status::RepoStatus;
use crate::{FailureKind, Git, GitError, template};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    /// Commit everything, pull, then push.
    Full,
    /// Bring in remote changes without committing anything.
    Pull,
}

/// What a pass is doing, for progress reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Committing,
    Pulling,
    Pushing,
}

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub method: SyncMethod,
    pub push: bool,
    /// The commit message template.
    pub message: String,
    pub date_format: String,
    pub hostname: String,
}

#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    /// Files in the commit this pass made (0 if it made none).
    pub committed: usize,
    /// Whether pulling changed HEAD.
    pub pulled: bool,
    pub pushed: bool,
    /// A pull-only pass found local and remote commits that need merging,
    /// and left that for the next commit-and-sync.
    pub deferred: bool,
    /// The repository afterwards.
    pub status: RepoStatus,
}

/// Runs one pass. `progress` is called as each phase starts.
pub fn run(
    git: &Git,
    kind: SyncKind,
    options: &SyncOptions,
    progress: &mut dyn FnMut(Phase),
) -> Result<SyncReport, GitError> {
    git.check_git_crypt()?;
    let status = git.status()?;
    paused(&status)?;
    let mut report = SyncReport::default();
    let head_before = git.head()?;

    if kind == SyncKind::Full {
        progress(Phase::Committing);
        git.stage_all()?;
        let status = git.status()?;
        let staged: Vec<_> = status.staged().cloned().collect();
        if !staged.is_empty() {
            let message = template::render(
                &options.message,
                &template::Context {
                    now: jiff::Zoned::now(),
                    date_format: &options.date_format,
                    hostname: &options.hostname,
                    files: &staged,
                },
            );
            if git.commit(&message)? {
                report.committed = staged.len();
            }
        }
    }

    let status = git.status()?;
    if status.upstream.is_none() {
        // A repository without a remote is fine: commits stay local. One
        // with a remote but no upstream needs the user to publish the branch.
        if !git.remotes()?.is_empty() && options.push {
            report.status = status;
            return Err(GitError::failed(
                FailureKind::NoUpstream,
                "This branch has no upstream branch yet",
            ));
        }
        report.status = status;
        return Ok(report);
    }

    progress(Phase::Pulling);
    match kind {
        SyncKind::Full => pull(git, options.method)?,
        SyncKind::Pull => {
            git.fetch()?;
            let status = git.status()?;
            if status.behind > 0 {
                let tracked_changes = status.entries.iter().any(|e| !e.is_untracked());
                if status.ahead == 0 {
                    match git.fast_forward() {
                        Ok(()) => {}
                        // Local edits to the same files: they'll be merged
                        // properly at the next commit-and-sync.
                        Err(e) if e.kind() == FailureKind::LocalChanges => report.deferred = true,
                        Err(e) => return Err(e),
                    }
                } else if tracked_changes {
                    report.deferred = true;
                } else {
                    pull(git, options.method)?;
                }
            }
        }
    }
    report.pulled = git.head()? != head_before && head_before.is_some();

    if kind == SyncKind::Full && options.push {
        let status = git.status()?;
        if status.ahead > 0 {
            progress(Phase::Pushing);
            git.push()?;
            report.pushed = true;
        }
    }
    report.status = git.status()?;
    Ok(report)
}

/// Pulls, turning a stop for conflicts into a conflict error.
fn pull(git: &Git, method: SyncMethod) -> Result<(), GitError> {
    match git.pull(method) {
        Ok(()) => Ok(()),
        Err(e) => {
            let status = git.status()?;
            if status.has_conflicts() || status.in_progress.is_some() {
                Err(conflict_error(&status))
            } else {
                Err(e)
            }
        }
    }
}

/// Sync waits while a merge or rebase is unfinished: the user resolves the
/// conflicts and commits from the Changes pane.
fn paused(status: &RepoStatus) -> Result<(), GitError> {
    if status.has_conflicts() || status.in_progress.is_some() {
        Err(conflict_error(status))
    } else {
        Ok(())
    }
}

fn conflict_error(status: &RepoStatus) -> GitError {
    let n = status.conflicts().count();
    let message = match n {
        0 => "A merge is waiting to be committed".to_owned(),
        1 => "1 file has conflicts".to_owned(),
        n => format!("{n} files have conflicts"),
    };
    GitError::failed(FailureKind::Conflict, message)
}
