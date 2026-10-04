use std::time::Duration;

/// Why a `git` command failed, worked out from its error output so the app
/// can say something useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The remote couldn't be reached.
    Offline,
    /// The remote refused our credentials, or wanted to ask for some.
    Auth,
    /// Another program holds `.git/index.lock`.
    Locked,
    /// The branch has no upstream to pull from or push to.
    NoUpstream,
    /// A merge or rebase stopped with conflicts.
    Conflict,
    /// Local changes would be overwritten.
    LocalChanges,
    /// The remote rejected the push (it has commits we don't).
    Rejected,
    /// Git doesn't know who the user is, so it can't commit.
    Identity,
    /// The repository uses git-crypt, which isn't installed or set up.
    GitCrypt,
    Other,
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("Git isn't installed")]
    NotInstalled,
    #[error("Git {found} is too old; Igneous needs {}.{} or newer", crate::MIN_VERSION.0, crate::MIN_VERSION.1)]
    TooOld { found: String },
    #[error("couldn't run Git: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Git didn't finish within {} seconds", .0.as_secs())]
    Timeout(Duration),
    #[error("{message}")]
    Failed {
        kind: FailureKind,
        /// A short, readable summary.
        message: String,
        /// Everything git printed to stderr.
        stderr: String,
    },
    #[error("unexpected output from Git: {0}")]
    Parse(String),
}

impl GitError {
    pub fn kind(&self) -> FailureKind {
        match self {
            GitError::Failed { kind, .. } => *kind,
            _ => FailureKind::Other,
        }
    }

    pub(crate) fn failed(kind: FailureKind, message: impl Into<String>) -> Self {
        GitError::Failed {
            kind,
            message: message.into(),
            stderr: String::new(),
        }
    }

    /// Builds an error from a failed command's output.
    pub(crate) fn from_output(stdout: &str, stderr: &str) -> Self {
        let kind = classify(&format!("{stderr}\n{stdout}"));
        let message = match kind {
            FailureKind::Offline => "Couldn't reach the remote".to_owned(),
            FailureKind::Auth => "The remote didn't accept your credentials".to_owned(),
            FailureKind::Locked => "Another program is using this repository".to_owned(),
            FailureKind::NoUpstream => "This branch has no upstream branch".to_owned(),
            FailureKind::Conflict => "Some notes have conflicts".to_owned(),
            FailureKind::LocalChanges => {
                "Local changes would be overwritten; they'll be committed first next time"
                    .to_owned()
            }
            FailureKind::Rejected => "The remote has changes that aren't here yet".to_owned(),
            FailureKind::Identity => {
                "Git doesn't know your name and email yet (set user.name and user.email)".to_owned()
            }
            FailureKind::GitCrypt => "git-crypt isn't set up for this repository".to_owned(),
            FailureKind::Other => summary(stderr).unwrap_or_else(|| "Git failed".to_owned()),
        };
        GitError::Failed {
            kind,
            message,
            stderr: stderr.to_owned(),
        }
    }
}

/// The most informative line of git's error output.
fn summary(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
        .collect();
    let line = lines
        .iter()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or(lines.first())?;
    let line = line
        .trim_start_matches("fatal:")
        .trim_start_matches("error:")
        .trim();
    let mut chars = line.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

fn classify(output: &str) -> FailureKind {
    let has = |needle: &str| output.contains(needle);
    if has("index.lock") && (has("File exists") || has("Another git process")) {
        FailureKind::Locked
    } else if has("CONFLICT") || has("Merge conflict") || has("could not apply") {
        FailureKind::Conflict
    } else if has("would be overwritten by merge")
        || has("would be overwritten by checkout")
        || has("cannot pull with rebase")
        || has("Please commit your changes or stash them")
    {
        FailureKind::LocalChanges
    } else if has("There is no tracking information")
        || has("has no upstream branch")
        || has("no upstream configured")
    {
        FailureKind::NoUpstream
    } else if has("Permission denied (publickey")
        || has("Authentication failed")
        || has("terminal prompts disabled")
        || has("could not read Username")
        || has("could not read Password")
        || has("Host key verification failed")
        || has("HTTP Basic: Access denied")
    {
        FailureKind::Auth
    } else if has("Could not resolve host")
        || has("Could not read from remote repository")
        || has("unable to access")
        || has("Connection timed out")
        || has("Connection refused")
        || has("Network is unreachable")
        || has("No route to host")
        || has("does not appear to be a git repository")
    {
        FailureKind::Offline
    } else if has("[rejected]") || has("non-fast-forward") || has("fetch first") {
        FailureKind::Rejected
    } else if has("Please tell me who you are") || has("unable to auto-detect email address") {
        FailureKind::Identity
    } else if has("git-crypt") {
        FailureKind::GitCrypt
    } else {
        FailureKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_failures() {
        let cases = [
            (
                "fatal: Unable to create '/v/.git/index.lock': File exists.",
                FailureKind::Locked,
            ),
            (
                "CONFLICT (content): Merge conflict in a.md",
                FailureKind::Conflict,
            ),
            (
                "ssh: Could not resolve hostname codeberg.org: Name or service not known\nfatal: Could not read from remote repository.",
                FailureKind::Offline,
            ),
            (
                "git@codeberg.org: Permission denied (publickey).\nfatal: Could not read from remote repository.",
                FailureKind::Auth,
            ),
            (
                "fatal: could not read Username for 'https://x': terminal prompts disabled",
                FailureKind::Auth,
            ),
            (
                "There is no tracking information for the current branch.",
                FailureKind::NoUpstream,
            ),
            (
                " ! [rejected]        main -> main (fetch first)",
                FailureKind::Rejected,
            ),
            (
                "Author identity unknown\n*** Please tell me who you are.",
                FailureKind::Identity,
            ),
            ("fatal: something odd", FailureKind::Other),
        ];
        for (stderr, kind) in cases {
            assert_eq!(classify(stderr), kind, "{stderr}");
        }
    }

    #[test]
    fn summary_prefers_fatal_lines() {
        assert_eq!(
            summary("hint: ignore me\nfatal: bad revision 'x'\n").as_deref(),
            Some("Bad revision 'x'")
        );
        assert_eq!(summary("\n\n"), None);
    }
}
