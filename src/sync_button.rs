//! The header bar's sync button: an icon for the sync state, and a popover
//! with the details and the sync actions.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use igneous_git::{FailureKind, Phase};

use crate::sync::{State, SyncService};

pub struct SyncButton {
    pub button: gtk::MenuButton,
    title: gtk::Label,
    detail: gtk::Label,
    publish: gtk::Button,
    spinner: adw::Spinner,
}

impl SyncButton {
    pub fn new(service: &Rc<SyncService>) -> Rc<Self> {
        let title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["heading"])
            .build();
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(36)
            .css_classes(["dim-label"])
            .build();
        let sync_now = gtk::Button::builder()
            .label("_Sync Now")
            .use_underline(true)
            .action_name("win.sync-now")
            .css_classes(["suggested-action", "pill"])
            .build();
        let pull = gtk::Button::builder()
            .label("_Pull")
            .use_underline(true)
            .action_name("win.pull")
            .css_classes(["pill"])
            .build();
        let changes = gtk::Button::builder()
            .label("Show _Changes")
            .use_underline(true)
            .action_name("win.show-changes")
            .css_classes(["pill"])
            .build();
        let publish = gtk::Button::builder()
            .label("P_ublish Branch")
            .use_underline(true)
            .action_name("win.publish-branch")
            .css_classes(["pill"])
            .visible(false)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .width_request(260)
            .build();
        content.append(&title);
        content.append(&detail);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(12)
            .build();
        for b in [&sync_now, &publish, &pull, &changes] {
            buttons.append(b);
        }
        content.append(&buttons);
        let popover = gtk::Popover::builder().child(&content).build();
        let button = gtk::MenuButton::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Sync")
            .popover(&popover)
            .visible(false)
            .build();
        let this = Rc::new(Self {
            button,
            title,
            detail,
            publish,
            spinner: adw::Spinner::new(),
        });
        let weak = Rc::downgrade(&this);
        let weak_service = Rc::downgrade(service);
        service.connect_changed(move || {
            if let (Some(this), Some(service)) = (weak.upgrade(), weak_service.upgrade()) {
                this.update(&service);
            }
        });
        // Keep "last synced" fresh while the popover is open.
        let weak = Rc::downgrade(&this);
        let weak_service = Rc::downgrade(service);
        popover.connect_show(move |_| {
            if let (Some(this), Some(service)) = (weak.upgrade(), weak_service.upgrade()) {
                this.update(&service);
            }
        });
        this.update(service);
        this
    }

    fn update(&self, service: &SyncService) {
        let state = service.state();
        self.button.set_visible(state != State::Unavailable);
        let status = service.status();
        let settings = service.settings();

        let (title, icon, accessible) = match &state {
            State::Unavailable => return,
            State::Idle => {
                let (ahead, behind) = status.as_ref().map_or((0, 0), |s| (s.ahead, s.behind));
                let title = match (ahead, behind) {
                    (0, 0) => "Up to Date".to_owned(),
                    (a, 0) => format!("{a} to Push"),
                    (0, b) => format!("{b} to Pull"),
                    (a, b) => format!("{a} to Push, {b} to Pull"),
                };
                (title, "view-refresh-symbolic", "Sync")
            }
            State::Busy(phase) => {
                let title = match phase {
                    None => "Syncing…",
                    Some(Phase::Committing) => "Committing…",
                    Some(Phase::Pulling) => "Pulling…",
                    Some(Phase::Pushing) => "Pushing…",
                };
                (title.to_owned(), "", "Syncing")
            }
            State::Paused => (
                "Sync Paused".to_owned(),
                "dialog-warning-symbolic",
                "Sync paused",
            ),
            State::Failed { kind, .. } => {
                let icon = match kind {
                    FailureKind::Offline => "network-offline-symbolic",
                    _ => "dialog-warning-symbolic",
                };
                ("Couldn’t Sync".to_owned(), icon, "Sync failed")
            }
        };
        if icon.is_empty() {
            self.button.set_child(Some(&self.spinner));
        } else {
            self.button.set_icon_name(icon);
        }
        self.button.set_tooltip_text(Some(accessible));
        self.title.set_label(&title);

        let mut lines = Vec::new();
        match &state {
            State::Failed { message, .. } => lines.push(message.clone()),
            State::Paused => {
                let n = status.as_ref().map_or(0, |s| s.conflicts().count());
                lines.push(match n {
                    0 => "A merge is waiting to be committed from the Changes pane.".to_owned(),
                    1 => "1 file has conflicts. Resolve it, then commit from the Changes pane."
                        .to_owned(),
                    n => format!(
                        "{n} files have conflicts. Resolve them, then commit from the Changes pane."
                    ),
                });
            }
            _ => {}
        }
        if let Some(status) = &status {
            let branch = status.branch.as_deref().unwrap_or("detached HEAD");
            lines.push(match &status.upstream {
                Some(upstream) => format!("{branch} → {upstream}"),
                None => format!("{branch} (no upstream branch)"),
            });
        }
        if let Some(time) = service.last_sync() {
            lines.push(format!("Last synced {}", relative_time(time)));
        }
        if !settings.enabled {
            lines.push("Automatic sync is off for this vault (Preferences → Plugins).".to_owned());
        }
        self.detail.set_label(&lines.join("\n"));
        let no_upstream = status.as_ref().is_some_and(|s| s.upstream.is_none())
            || matches!(
                state,
                State::Failed {
                    kind: FailureKind::NoUpstream,
                    ..
                }
            );
        self.publish.set_visible(no_upstream);
    }
}

/// "just now", "5 minutes ago", "3 hours ago", or a date.
pub fn relative_time(unix: i64) -> String {
    let now = glib::DateTime::now_utc().map_or(unix, |t| t.to_unix());
    let secs = (now - unix).max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..120 => "a minute ago".to_owned(),
        120..3600 => format!("{} minutes ago", secs / 60),
        3600..7200 => "an hour ago".to_owned(),
        7200..86_400 => format!("{} hours ago", secs / 3600),
        _ => glib::DateTime::from_unix_local(unix)
            .and_then(|t| t.format("%-d %B %Y"))
            .map_or_else(|_| String::new(), |s| s.to_string()),
    }
}
