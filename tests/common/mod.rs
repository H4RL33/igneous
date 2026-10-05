//! Helpers shared by the UI test files. Every test file runs in the sealed
//! headless session (build-aux/run-ui-tests.sh).

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use igneous::Window;
use igneous_core::VaultPath;

pub const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vaults/basic");

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A fresh copy of the fixture vault, optionally with an autosave delay.
pub fn vault(autosave_ms: Option<u32>) -> tempfile::TempDir {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
    }
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE), dir.path());
    if let Some(ms) = autosave_ms {
        std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
        std::fs::write(
            dir.path().join(".igneous/vault.json"),
            format!("{{\"version\":1,\"editor\":{{\"autosaveDelayMs\":{ms}}}}}"),
        )
        .unwrap();
    }
    dir
}

/// Opens and shows a window: GTK only closes windows that have been shown.
pub fn open(dir: &tempfile::TempDir) -> Window {
    let window = Window::for_vault(dir.path()).unwrap();
    window.present();
    window
}

pub fn p(s: &str) -> VaultPath {
    VaultPath::new(s).unwrap()
}

pub async fn wait(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The fixture vault as a repository whose `main` is pushed to a bare
/// remote. The second directory holds the remote and any other clones.
pub fn synced_vault() -> (tempfile::TempDir, tempfile::TempDir) {
    let dir = vault(Some(100));
    let remotes = tempfile::tempdir().unwrap();
    git(remotes.path(), &["init", "-q", "--bare", "remote.git"]);
    let remote = remotes.path().join("remote.git");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    git(
        dir.path(),
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(dir.path(), &["push", "-q", "-u", "origin", "main"]);
    (dir, remotes)
}

/// Another computer: a clone of the remote.
pub fn other_clone(remotes: &tempfile::TempDir) -> std::path::PathBuf {
    let path = remotes.path().join("other");
    if !path.exists() {
        git(remotes.path(), &["clone", "-q", "remote.git", "other"]);
    }
    path
}

pub fn remote_log(remotes: &tempfile::TempDir) -> String {
    git(
        &remotes.path().join("remote.git"),
        &["log", "--format=%s", "main"],
    )
}

/// Waits up to `ms` for `f` to hold.
pub async fn until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..ms / 50 {
        if f() {
            return true;
        }
        wait(50).await;
    }
    f()
}

pub fn save_png(window: &gtk::Window, out: &str) {
    // Paint the whole window: header bars are translucent over its background.
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let texture = window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None);
    texture.save_to_png(out).unwrap();
}

/// How dark the darkest pixel drawn over `widget` is, 0 (white) to 255
/// (black), from the whole window as it's drawn now.
#[allow(dead_code)]
pub fn darkest_in(widget: &gtk::Widget) -> u8 {
    let window = widget.root().and_downcast::<gtk::Window>().unwrap();
    let paintable = gtk::WidgetPaintable::new(Some(&window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    let texture = window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None);
    let mut downloader = gtk::gdk::TextureDownloader::new(&texture);
    downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let bounds = widget.compute_bounds(&window).unwrap();
    let scale = texture.width() as f32 / window.width() as f32;
    let (x0, y0) = ((bounds.x() * scale) as usize, (bounds.y() * scale) as usize);
    let (x1, y1) = (
        ((bounds.x() + bounds.width()) * scale) as usize,
        ((bounds.y() + bounds.height()) * scale) as usize,
    );
    let mut darkest = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let at = y * stride + x * 4;
            let lightness =
                (u16::from(bytes[at]) + u16::from(bytes[at + 1]) + u16::from(bytes[at + 2])) / 3;
            darkest = darkest.max(255 - lightness as u8);
        }
    }
    darkest
}

/// Real pointer and keyboard input, through the remote desktop API of the
/// sealed session's mutter. Events then go through GTK as a person's would,
/// which `emit_by_name` can't imitate. Windows used with it must be
/// maximized, so their coordinates are the screen's.
#[allow(dead_code)]
pub struct RemoteInput {
    bus: gtk::gio::DBusConnection,
    session: String,
}

#[allow(dead_code)]
impl RemoteInput {
    const SESSION: &str = "org.gnome.Mutter.RemoteDesktop.Session";

    pub async fn new() -> Self {
        let bus = gtk::gio::bus_get_future(gtk::gio::BusType::Session)
            .await
            .expect("the session bus");
        let reply = bus
            .call_future(
                Some("org.gnome.Mutter.RemoteDesktop"),
                "/org/gnome/Mutter/RemoteDesktop",
                "org.gnome.Mutter.RemoteDesktop",
                "CreateSession",
                None,
                None,
                gtk::gio::DBusCallFlags::NONE,
                5000,
            )
            .await
            .expect("mutter's remote desktop (run in build-aux/headless-session.sh)");
        let session = reply.child_value(0).str().unwrap().to_owned();
        let input = Self { bus, session };
        input.call("Start", None).await;
        // The first key after starting is lost while the keymap is set up.
        input.key(0xffe1).await;
        input
    }

    async fn call(&self, method: &str, args: Option<&glib::Variant>) {
        self.bus
            .call_future(
                Some("org.gnome.Mutter.RemoteDesktop"),
                &self.session,
                Self::SESSION,
                method,
                args,
                None,
                gtk::gio::DBusCallFlags::NONE,
                5000,
            )
            .await
            .unwrap_or_else(|e| panic!("{method}: {e}"));
    }

    /// Clicks the middle of `widget` (or `x` pixels in from its start).
    pub async fn click(&self, widget: &gtk::Widget, x: Option<f32>) {
        self.point_at(widget, x).await;
        for pressed in [true, false] {
            self.call("NotifyPointerButton", Some(&(272i32, pressed).to_variant()))
                .await;
        }
        wait(150).await;
    }

    /// Moves the pointer over `widget`, `x` from its left (or its middle).
    pub async fn point_at(&self, widget: &gtk::Widget, x: Option<f32>) {
        let root = widget.root().expect("a widget on screen");
        // Only a maximized window's coordinates are the screen's, and
        // maximizing takes a moment.
        let window = root.clone().downcast::<gtk::Window>().unwrap();
        assert!(
            until(3000, || window.is_maximized()).await,
            "maximize the window first"
        );
        wait(200).await;
        let bounds = widget.compute_bounds(&root).unwrap();
        let (x, y) = (
            f64::from(bounds.x() + x.unwrap_or(bounds.width() / 2.0)),
            f64::from(bounds.y() + bounds.height() / 2.0),
        );
        // Relative moves are accelerated, so steer: move, see where GTK says
        // the pointer is, and correct until it's there.
        let at = std::rc::Rc::new(std::cell::Cell::new(None::<(f64, f64)>));
        let motion = gtk::EventControllerMotion::new();
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let at = at.clone();
            motion.connect_motion(move |_, x, y| at.set(Some((x, y))));
        }
        window.add_controller(motion.clone());
        self.call(
            "NotifyPointerMotionRelative",
            Some(&(-5000.0f64, -5000.0f64).to_variant()),
        )
        .await;
        let (mut dx, mut dy) = (x, y);
        for _ in 0..20 {
            self.call("NotifyPointerMotionRelative", Some(&(dx, dy).to_variant()))
                .await;
            wait(30).await;
            let Some((px, py)) = at.get() else {
                continue;
            };
            if (x - px).abs() < 1.5 && (y - py).abs() < 1.5 {
                break;
            }
            (dx, dy) = ((x - px) / 2.0, (y - py) / 2.0);
        }
        window.remove_controller(&motion);
        wait(50).await;
    }

    /// Presses and releases the key with X keysym `keysym`.
    pub async fn key(&self, keysym: u32) {
        for pressed in [true, false] {
            self.call(
                "NotifyKeyboardKeysym",
                Some(&(keysym, pressed).to_variant()),
            )
            .await;
        }
        wait(30).await;
    }

    /// Types lower-case ASCII text.
    pub async fn type_text(&self, text: &str) {
        for c in text.chars() {
            self.key(c as u32).await;
        }
        wait(100).await;
    }

    /// Presses Enter.
    pub async fn enter(&self) {
        self.key(0xff0d).await;
        wait(100).await;
    }
}
