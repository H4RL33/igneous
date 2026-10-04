//! Git sync for one window: a worker thread that runs `git`, the schedule
//! (pull on open, commit-and-sync and pull intervals), and the state shown by
//! the sync button, the Changes pane and the conflict banner.
//!
//! Every Git command runs on the worker, one at a time, so a slow push never
//! blocks the window and two commands never race for the index lock.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gio, glib};
use igneous_core::settings::{self as vault_settings, GitSettings};
use igneous_git::sync::{self, Phase, SyncKind, SyncOptions, SyncReport};
use igneous_git::{FailureKind, Git, GitError, RepoStatus};

use crate::worker::Worker;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Not a Git repository (or Git can't run); sync UI stays hidden.
    Unavailable,
    Idle,
    /// A pass is running; the phase once it's known.
    Busy(Option<Phase>),
    /// Conflicts or an unfinished merge: nothing runs until the user commits.
    Paused,
    Failed {
        kind: FailureKind,
        message: String,
    },
}

pub struct SyncService {
    igneous_dir: PathBuf,
    git: RefCell<Option<Git>>,
    worker: RefCell<Option<Worker<Git>>>,
    settings: RefCell<GitSettings>,
    settings_error: RefCell<Option<String>>,
    state: RefCell<State>,
    status: RefCell<Option<RepoStatus>>,
    /// When the last pass finished, as Unix seconds.
    last_sync: Cell<Option<i64>>,
    running: Cell<bool>,
    timers: RefCell<Vec<glib::SourceId>>,
    refresh_timer: RefCell<Option<glib::SourceId>>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
    before_commit: RefCell<Option<Box<dyn Fn()>>>,
}

impl SyncService {
    /// Looks for a repository at `root` in the background, then starts the
    /// schedule if sync is turned on for the vault.
    pub fn new(root: &Path, igneous_dir: PathBuf) -> Rc<Self> {
        let (settings, settings_error) = match vault_settings::load::<GitSettings>(&igneous_dir) {
            Ok(settings) => (settings, None),
            Err(e) => (GitSettings::default(), Some(e.to_string())),
        };
        let service = Rc::new(Self {
            igneous_dir,
            git: RefCell::default(),
            worker: RefCell::default(),
            settings: RefCell::new(settings),
            settings_error: RefCell::new(settings_error),
            state: RefCell::new(State::Unavailable),
            status: RefCell::default(),
            last_sync: Cell::default(),
            running: Cell::default(),
            timers: RefCell::default(),
            refresh_timer: RefCell::default(),
            listeners: RefCell::default(),
            before_commit: RefCell::default(),
        });
        let root = root.to_path_buf();
        let weak = Rc::downgrade(&service);
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || Git::discover(&root)).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            match found {
                Ok(Ok(Some(git))) => service.attach(git),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => tracing::warn!(%e, "Git sync unavailable"),
                Err(_) => tracing::warn!("Git discovery panicked"),
            }
        });
        service
    }

    fn attach(self: &Rc<Self>, git: Git) {
        let owned = git.clone();
        self.worker
            .replace(Some(Worker::new("igneous-git", move || owned)));
        self.git.replace(Some(git));
        self.state.replace(State::Idle);
        self.notify();
        self.refresh_now();
        self.schedule();
        let settings = self.settings();
        if settings.enabled && settings.pull_on_open {
            let service = self.clone();
            glib::spawn_future_local(async move {
                service.run(SyncKind::Pull, false).await;
            });
        }
    }

    pub fn is_available(&self) -> bool {
        self.git.borrow().is_some()
    }

    pub fn git(&self) -> Option<Git> {
        self.git.borrow().clone()
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    pub fn status(&self) -> Option<RepoStatus> {
        self.status.borrow().clone()
    }

    pub fn last_sync(&self) -> Option<i64> {
        self.last_sync.get()
    }

    pub fn settings(&self) -> GitSettings {
        self.settings.borrow().clone()
    }

    /// Saves new settings to `.igneous/git.json` and restarts the schedule.
    pub fn set_settings(self: &Rc<Self>, settings: GitSettings) -> Result<(), String> {
        if let Some(error) = self.settings_error.borrow().as_ref() {
            return Err(format!(
                "git.json can't be read, so it wasn't changed: {error}"
            ));
        }
        vault_settings::save(&self.igneous_dir, &settings).map_err(|e| e.to_string())?;
        self.settings.replace(settings);
        self.schedule();
        self.notify();
        Ok(())
    }

    /// Called whenever the state or status changes.
    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Called before every commit, to save open notes.
    pub fn set_before_commit(&self, f: impl Fn() + 'static) {
        self.before_commit.replace(Some(Box::new(f)));
    }

    fn notify(&self) {
        for listener in self.listeners.borrow().iter() {
            listener();
        }
    }

    fn set_state(&self, state: State) {
        self.state.replace(state);
        self.notify();
    }

    /// Runs `f` on the Git thread. `None` if the vault isn't a repository.
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Git) -> R + Send + 'static,
    ) -> Option<R> {
        let worker = self.worker.borrow().clone()?;
        worker.call(|git| f(git)).await
    }

    // --- status ----------------------------------------------------------------

    /// Re-reads the status soon, coalescing bursts of calls.
    pub fn refresh(self: &Rc<Self>) {
        if !self.is_available() {
            return;
        }
        if let Some(id) = self.refresh_timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(400), move || {
            if let Some(service) = weak.upgrade() {
                service.refresh_timer.take();
                service.refresh_now();
            }
        });
        self.refresh_timer.replace(Some(id));
    }

    pub fn refresh_now(self: &Rc<Self>) {
        let service = self.clone();
        glib::spawn_future_local(async move {
            let Some(result) = service.call(|git| git.status()).await else {
                return;
            };
            match result {
                Ok(status) => service.set_status(status),
                Err(e) => tracing::warn!(%e, "couldn't read the Git status"),
            }
        });
    }

    fn set_status(&self, status: RepoStatus) {
        let blocked = status.has_conflicts() || status.in_progress.is_some();
        self.status.replace(Some(status));
        if !self.running.get() {
            let state = self.state();
            if blocked {
                self.state.replace(State::Paused);
            } else if state == State::Paused
                || matches!(
                    state,
                    State::Failed {
                        kind: FailureKind::Conflict,
                        ..
                    }
                )
            {
                self.state.replace(State::Idle);
            }
        }
        self.notify();
    }

    /// Whether automatic passes are held back by conflicts.
    pub fn is_paused(&self) -> bool {
        self.state() == State::Paused
    }

    // --- passes ------------------------------------------------------------------

    fn schedule(self: &Rc<Self>) {
        for id in self.timers.take() {
            id.remove();
        }
        let settings = self.settings();
        if !settings.enabled || !self.is_available() {
            return;
        }
        let mut timers = Vec::new();
        for (minutes, kind) in [
            (settings.sync_interval, SyncKind::Full),
            (settings.pull_interval, SyncKind::Pull),
        ] {
            if minutes == 0 {
                continue;
            }
            let weak = Rc::downgrade(self);
            timers.push(glib::timeout_add_seconds_local(
                minutes.saturating_mul(60),
                move || {
                    let Some(service) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    glib::spawn_future_local(async move {
                        service.run(kind, false).await;
                    });
                    glib::ControlFlow::Continue
                },
            ));
        }
        self.timers.replace(timers);
    }

    /// Runs one pass. Automatic passes (`manual` false) skip while another
    /// pass runs or sync is paused. Returns `None` if nothing ran.
    pub async fn run(
        self: Rc<Self>,
        kind: SyncKind,
        manual: bool,
    ) -> Option<Result<SyncReport, GitError>> {
        if !self.is_available() || self.running.get() || (!manual && self.is_paused()) {
            return None;
        }
        if kind == SyncKind::Full
            && let Some(flush) = self.before_commit.borrow().as_ref()
        {
            flush();
        }
        self.running.set(true);
        self.set_state(State::Busy(None));
        let settings = self.settings();
        let options = SyncOptions {
            method: settings.method,
            push: settings.push,
            message: settings.commit_message.clone(),
            date_format: settings.date_format.clone(),
            hostname: glib::host_name().to_string(),
        };
        let (phases, phase_receiver) = async_channel::unbounded();
        let watcher = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(phase) = phase_receiver.recv().await {
                if watcher.running.get() {
                    watcher.set_state(State::Busy(Some(phase)));
                }
            }
        });
        let result = self
            .call(move |git| {
                sync::run(git, kind, &options, &mut |phase| {
                    let _ = phases.send_blocking(phase);
                })
            })
            .await;
        self.running.set(false);
        let Some(result) = result else {
            self.set_state(State::Idle);
            return None;
        };
        match &result {
            Ok(report) => {
                self.last_sync
                    .set(Some(glib::DateTime::now_utc().map_or(0, |t| t.to_unix())));
                self.state.replace(State::Idle);
                self.set_status(report.status.clone());
            }
            Err(e) if e.kind() == FailureKind::Conflict => {
                self.state.replace(State::Paused);
                self.refresh_now();
            }
            Err(e) => {
                self.set_state(State::Failed {
                    kind: e.kind(),
                    message: e.to_string(),
                });
                self.refresh_now();
            }
        }
        Some(result)
    }

    /// Pushes the current branch to its only remote and tracks it there.
    pub async fn publish(self: Rc<Self>) -> Option<Result<(), GitError>> {
        let result = self
            .call(|git| {
                let remotes = git.remotes()?;
                let remote = match remotes.as_slice() {
                    [only] => only.clone(),
                    _ if remotes.iter().any(|r| r == "origin") => "origin".to_owned(),
                    [] => {
                        return Err(GitError::Failed {
                            kind: FailureKind::NoUpstream,
                            message: "This repository has no remote".into(),
                            stderr: String::new(),
                        });
                    }
                    _ => {
                        return Err(GitError::Failed {
                            kind: FailureKind::NoUpstream,
                            message: "This repository has several remotes; publish the branch \
                                      with `git push -u <remote> HEAD`"
                                .into(),
                            stderr: String::new(),
                        });
                    }
                };
                git.publish(&remote)
            })
            .await?;
        if result.is_ok() && matches!(self.state(), State::Failed { .. }) {
            self.state.replace(State::Idle);
        }
        self.refresh_now();
        Some(result)
    }
}

impl Drop for SyncService {
    fn drop(&mut self) {
        for id in self.timers.take() {
            id.remove();
        }
        if let Some(id) = self.refresh_timer.take() {
            id.remove();
        }
    }
}

/// A short description of a pass's result, for a toast.
pub fn describe(report: &SyncReport) -> String {
    let mut parts = Vec::new();
    match report.committed {
        0 => {}
        1 => parts.push("committed 1 file".to_owned()),
        n => parts.push(format!("committed {n} files")),
    }
    if report.pulled {
        parts.push("pulled changes".to_owned());
    }
    if report.pushed {
        parts.push("pushed".to_owned());
    }
    if report.deferred {
        parts.push("remote changes will be merged at the next sync".to_owned());
    }
    if parts.is_empty() {
        return "Already up to date".to_owned();
    }
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}
