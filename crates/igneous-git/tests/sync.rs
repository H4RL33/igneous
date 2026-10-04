//! Sync against a local bare remote. Every command runs with a private Git
//! configuration, so the user's own settings (signing, hooks, editors) never
//! come into play.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use igneous_core::settings::SyncMethod;
use igneous_git::{FailureKind, Git, SyncKind, SyncOptions, sync};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    remote: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("gitconfig"),
            "[user]\n\tname = Test\n\temail = test@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n",
        )
        .unwrap();
        let remote = dir.path().join("remote.git");
        let fixture = Fixture { dir, remote };
        fixture.git(fixture.dir.path(), &["init", "-q", "--bare", "remote.git"]);
        fixture
    }

    fn envs(&self) -> Vec<(OsString, OsString)> {
        vec![
            (
                "GIT_CONFIG_GLOBAL".into(),
                self.dir.path().join("gitconfig").into(),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("HOME".into(), self.dir.path().into()),
        ]
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .envs(self.envs())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A new clone of the remote, with an initial commit pushed if the remote
    /// is empty.
    fn clone(&self, name: &str) -> (PathBuf, Git) {
        let path = self.dir.path().join(name);
        self.git(
            self.dir.path(),
            &["clone", "-q", self.remote.to_str().unwrap(), name],
        );
        if self.git(&path, &["branch", "-r"]).trim().is_empty() {
            std::fs::write(path.join("Home.md"), "# Home\n").unwrap();
            self.git(&path, &["add", "-A"]);
            self.git(&path, &["commit", "-q", "-m", "init"]);
            self.git(&path, &["push", "-q", "-u", "origin", "main"]);
        }
        let git = Git::discover_with(&path, self.envs()).unwrap().unwrap();
        (path, git)
    }
}

fn options(method: SyncMethod) -> SyncOptions {
    SyncOptions {
        method,
        push: true,
        message: "vault backup: {{numFiles}} files".into(),
        date_format: "YYYY-MM-DD".into(),
        hostname: "test".into(),
    }
}

fn full(git: &Git, method: SyncMethod) -> Result<sync::SyncReport, igneous_git::GitError> {
    sync::run(git, SyncKind::Full, &options(method), &mut |_| {})
}

fn pull(git: &Git) -> Result<sync::SyncReport, igneous_git::GitError> {
    sync::run(
        git,
        SyncKind::Pull,
        &options(SyncMethod::Merge),
        &mut |_| {},
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Git::discover(dir.path()).unwrap().is_none());
}

#[test]
fn clean_sync() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    let (b, git_b) = f.clone("b");

    std::fs::write(a.join("Ideas.md"), "an idea\n").unwrap();
    let mut phases = Vec::new();
    let report = sync::run(
        &git_a,
        SyncKind::Full,
        &options(SyncMethod::Merge),
        &mut |p| phases.push(p),
    )
    .unwrap();
    assert_eq!(report.committed, 1);
    assert!(report.pushed);
    assert_eq!(
        phases,
        [
            sync::Phase::Committing,
            sync::Phase::Pulling,
            sync::Phase::Pushing
        ]
    );
    assert_eq!(report.status.ahead, 0);
    assert!(!report.status.is_dirty());
    let subject = f.git(&a, &["log", "-1", "--format=%s"]);
    assert_eq!(subject.trim(), "vault backup: 1 files");

    let report = pull(&git_b).unwrap();
    assert!(report.pulled);
    assert_eq!(read(&b.join("Ideas.md")), "an idea\n");

    // Nothing to do: no commit, no push.
    let report = full(&git_a, SyncMethod::Merge).unwrap();
    assert_eq!(report.committed, 0);
    assert!(!report.pushed && !report.pulled);
}

#[test]
fn divergent_merge_and_rebase() {
    for method in [SyncMethod::Merge, SyncMethod::Rebase] {
        let f = Fixture::new();
        let (a, git_a) = f.clone("a");
        let (b, git_b) = f.clone("b");
        std::fs::write(a.join("A.md"), "from a\n").unwrap();
        std::fs::write(b.join("B.md"), "from b\n").unwrap();
        full(&git_a, method).unwrap();
        let report = full(&git_b, method).unwrap();
        assert!(report.pulled && report.pushed, "{method:?}");
        assert_eq!(read(&b.join("A.md")), "from a\n");
        let parents = f.git(&b, &["log", "-1", "--format=%p"]);
        let merge = parents.split_whitespace().count() == 2;
        assert_eq!(merge, method == SyncMethod::Merge, "{method:?}");

        pull(&git_a).unwrap();
        assert_eq!(read(&a.join("B.md")), "from b\n");
    }
}

#[test]
fn conflicts_pause_sync_until_committed() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    let (b, git_b) = f.clone("b");
    std::fs::write(a.join("Home.md"), "# Home from a\n").unwrap();
    std::fs::write(b.join("Home.md"), "# Home from b\n").unwrap();
    full(&git_a, SyncMethod::Merge).unwrap();

    let error = full(&git_b, SyncMethod::Merge).unwrap_err();
    assert_eq!(error.kind(), FailureKind::Conflict);
    let status = git_b.status().unwrap();
    assert_eq!(status.conflicts().count(), 1);
    assert!(read(&b.join("Home.md")).contains("<<<<<<<"));

    // Paused: neither kind of pass runs, and nothing is touched.
    assert_eq!(
        full(&git_b, SyncMethod::Merge).unwrap_err().kind(),
        FailureKind::Conflict
    );
    assert_eq!(pull(&git_b).unwrap_err().kind(), FailureKind::Conflict);
    assert!(read(&b.join("Home.md")).contains("<<<<<<<"));

    // The user resolves, stages and commits.
    std::fs::write(b.join("Home.md"), "# Home from both\n").unwrap();
    git_b.stage(&["Home.md"]).unwrap();
    git_b.conclude().unwrap();
    let report = full(&git_b, SyncMethod::Merge).unwrap();
    assert!(report.pushed);
    pull(&git_a).unwrap();
    assert_eq!(read(&a.join("Home.md")), "# Home from both\n");
}

#[test]
fn unreachable_remote() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    let gone = f.dir.path().join("gone.git");
    f.git(&a, &["remote", "set-url", "origin", gone.to_str().unwrap()]);
    std::fs::write(a.join("Offline.md"), "written offline\n").unwrap();
    let error = full(&git_a, SyncMethod::Merge).unwrap_err();
    assert_eq!(error.kind(), FailureKind::Offline, "{error:?}");
    // The commit still happened, so nothing is lost.
    let subject = f.git(&a, &["log", "-1", "--format=%s"]);
    assert!(subject.starts_with("vault backup"));
}

#[test]
fn missing_upstream_until_published() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    f.git(&a, &["checkout", "-q", "-b", "drafts"]);
    std::fs::write(a.join("Draft.md"), "draft\n").unwrap();
    let error = full(&git_a, SyncMethod::Merge).unwrap_err();
    assert_eq!(error.kind(), FailureKind::NoUpstream);
    git_a.publish("origin").unwrap();
    let status = git_a.status().unwrap();
    assert_eq!(status.upstream.as_deref(), Some("origin/drafts"));
    assert_eq!(full(&git_a, SyncMethod::Merge).unwrap().committed, 0);
}

#[test]
fn local_only_repositories_just_commit() {
    let f = Fixture::new();
    let dir = f.dir.path().join("local");
    std::fs::create_dir(&dir).unwrap();
    f.git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("Note.md"), "hello\n").unwrap();
    let git = Git::discover_with(&dir, f.envs()).unwrap().unwrap();
    let report = full(&git, SyncMethod::Merge).unwrap();
    assert_eq!(report.committed, 1);
    assert!(!report.pushed);
}

#[test]
fn pull_only_never_merges_over_local_edits() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    let (b, git_b) = f.clone("b");
    std::fs::write(a.join("Home.md"), "# Home from a\n").unwrap();
    full(&git_a, SyncMethod::Merge).unwrap();

    std::fs::write(b.join("Home.md"), "# unsaved idea in b\n").unwrap();
    let report = pull(&git_b).unwrap();
    assert!(report.deferred && !report.pulled);
    assert_eq!(read(&b.join("Home.md")), "# unsaved idea in b\n");

    // Untouched files fast-forward even with other local edits.
    std::fs::write(a.join("Other.md"), "other\n").unwrap();
    full(&git_a, SyncMethod::Merge).unwrap();
    std::fs::write(b.join("Home.md"), "# Home\n").unwrap();
    std::fs::write(b.join("Mine.md"), "mine\n").unwrap();
    f.git(&b, &["checkout", "-q", "--", "Home.md"]);
    let report = pull(&git_b).unwrap();
    assert!(report.pulled);
    assert_eq!(read(&b.join("Other.md")), "other\n");
    assert_eq!(read(&b.join("Mine.md")), "mine\n");
}

#[test]
fn waits_for_the_index_lock() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    let lock = a.join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let release = lock.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        std::fs::remove_file(release).unwrap();
    });
    std::fs::write(a.join("Locked.md"), "x\n").unwrap();
    git_a.stage_all().unwrap();
    releaser.join().unwrap();

    // Held for good: give up with a clear error.
    std::fs::write(&lock, "").unwrap();
    std::fs::write(a.join("Locked.md"), "y\n").unwrap();
    assert_eq!(git_a.stage_all().unwrap_err().kind(), FailureKind::Locked);
    std::fs::remove_file(&lock).unwrap();
}

#[test]
fn git_crypt_without_setup_refuses() {
    let f = Fixture::new();
    let (a, git_a) = f.clone("a");
    std::fs::write(
        a.join(".gitattributes"),
        "* filter=git-crypt diff=git-crypt\n",
    )
    .unwrap();
    std::fs::write(a.join("Secret.md"), "plaintext\n").unwrap();
    let error = full(&git_a, SyncMethod::Merge).unwrap_err();
    assert_eq!(error.kind(), FailureKind::GitCrypt);
    // Nothing was committed or pushed.
    assert_eq!(f.git(&a, &["log", "--format=%s"]).trim(), "init");
}

#[test]
fn vaults_inside_a_larger_repository() {
    let f = Fixture::new();
    let (a, _) = f.clone("a");
    std::fs::create_dir_all(a.join("notes/Daily")).unwrap();
    std::fs::write(a.join("notes/Daily/Today.md"), "today\n").unwrap();
    std::fs::write(a.join("outside.txt"), "not in the vault\n").unwrap();
    let git = Git::discover_with(&a.join("notes"), f.envs())
        .unwrap()
        .unwrap();
    let status = git.status().unwrap();
    let paths: Vec<_> = status.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["notes/Daily/Today.md"]);
    assert_eq!(git.to_vault("notes/Daily/Today.md"), Some("Daily/Today.md"));
    assert_eq!(git.to_vault("outside.txt"), None);
    assert_eq!(git.to_repo("Daily/Today.md"), "notes/Daily/Today.md");

    full(&git, SyncMethod::Merge).unwrap();
    let left = f.git(&a, &["status", "--porcelain"]);
    assert_eq!(left.trim(), "?? outside.txt");
}

#[test]
fn staging_diffs_and_history() {
    let f = Fixture::new();
    let (a, git) = f.clone("a");
    std::fs::write(a.join("Home.md"), "# Home\nmore\n").unwrap();
    std::fs::write(a.join("New.md"), "new\n").unwrap();

    let diff = git.diff("Home.md", false, false).unwrap();
    assert!(diff.contains("+more"));
    let diff = git.diff("New.md", false, true).unwrap();
    assert!(diff.contains("+new"));

    git.stage(&["Home.md"]).unwrap();
    let status = git.status().unwrap();
    assert!(status.entry("Home.md").unwrap().is_staged());
    assert!(git.diff("Home.md", true, false).unwrap().contains("+more"));
    git.unstage(&["Home.md"]).unwrap();
    assert!(!git.status().unwrap().entry("Home.md").unwrap().is_staged());

    git.discard(&["Home.md"]).unwrap();
    assert_eq!(read(&a.join("Home.md")), "# Home\n");
    assert!(git.status().unwrap().entry("New.md").is_some());

    // History follows renames, and old versions open by their old name.
    f.git(&a, &["mv", "Home.md", "Start.md"]);
    git.stage_all().unwrap();
    git.commit("rename").unwrap();
    let log = git.log("Start.md", 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "rename");
    assert_eq!(log[0].path, "Start.md");
    assert_eq!(log[1].subject, "init");
    assert_eq!(log[1].path, "Home.md");
    let old = git.show(&log[1].hash, &log[1].path).unwrap();
    assert_eq!(old, b"# Home\n");
}

#[test]
fn unstaging_before_the_first_commit() {
    let f = Fixture::new();
    let dir = f.dir.path().join("fresh");
    std::fs::create_dir(&dir).unwrap();
    f.git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("Note.md"), "hello\n").unwrap();
    let git = Git::discover_with(&dir, f.envs()).unwrap().unwrap();
    git.stage(&["Note.md"]).unwrap();
    assert!(git.status().unwrap().entries[0].is_staged());
    git.unstage(&["Note.md"]).unwrap();
    assert!(git.status().unwrap().entries[0].is_untracked());
}
