//! M0 prototype: is Obsidian-style Live Preview workable on GtkSourceView?
//!
//! ```text
//! live-preview-spike [FILE] [--conceal invisible|zerosize] [--mode live|source|reading]
//!                    [--sample KIB] [--bench KEYS] [--nav] [--soak SECS] [--seed N]
//!                    [--screenshot PNG] [--cursor BYTE] [--scheme light|dark] [--baseline]
//! ```
//!
//! Without FILE, a sample note is generated. Throwaway code: see
//! docs/decisions/0001-editor.md for what it showed.

mod controller;
mod harness;
mod rangeset;
mod tags;
mod view;

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

use controller::{Controller, Mode};
use harness::Task;
use tags::Concealment;

#[derive(Debug, Clone)]
struct Args {
    file: Option<PathBuf>,
    concealment: Concealment,
    mode: Mode,
    sample_kib: usize,
    scheme: adw::ColorScheme,
    baseline: bool,
    task: Task,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        file: None,
        concealment: Concealment::ZeroSize,
        mode: Mode::Live,
        sample_kib: 50,
        scheme: adw::ColorScheme::Default,
        baseline: false,
        task: Task::Interactive,
    };
    let mut seed = 1;
    let mut cursor = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--conceal" => {
                args.concealment = match value()?.as_str() {
                    "invisible" => Concealment::Invisible,
                    "zerosize" => Concealment::ZeroSize,
                    other => return Err(format!("unknown concealment {other}")),
                }
            }
            "--mode" => {
                args.mode = match value()?.as_str() {
                    "live" => Mode::Live,
                    "source" => Mode::Source,
                    "reading" => Mode::Reading,
                    other => return Err(format!("unknown mode {other}")),
                }
            }
            "--scheme" => {
                args.scheme = match value()?.as_str() {
                    "light" => adw::ColorScheme::ForceLight,
                    "dark" => adw::ColorScheme::ForceDark,
                    _ => adw::ColorScheme::Default,
                }
            }
            "--sample" => args.sample_kib = value()?.parse().map_err(|e| format!("{e}"))?,
            "--bench" => {
                args.task = Task::Bench {
                    keys: value()?.parse().map_err(|e| format!("{e}"))?,
                }
            }
            "--nav" => args.task = Task::Nav,
            "--baseline" => args.baseline = true,
            "--soak" => {
                args.task = Task::Soak {
                    secs: value()?.parse().map_err(|e| format!("{e}"))?,
                    seed,
                }
            }
            "--seed" => {
                seed = value()?.parse().map_err(|e| format!("{e}"))?;
                if let Task::Soak { seed: s, .. } = &mut args.task {
                    *s = seed;
                }
            }
            "--screenshot" => {
                args.task = Task::Screenshot {
                    path: value()?.into(),
                    cursor,
                }
            }
            "--cursor" => {
                cursor = Some(value()?.parse().map_err(|e| format!("{e}"))?);
                if let Task::Screenshot { cursor: c, .. } = &mut args.task {
                    *c = cursor;
                }
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            file => args.file = Some(file.into()),
        }
    }
    Ok(args)
}

fn main() -> glib::ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let app = adw::Application::builder()
        .application_id("io.github.h4rl33.igneous.Spike")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        if let Err(e) = build(app, &args) {
            eprintln!("{e}");
            app.quit();
        }
    });
    app.run_with_args::<&str>(&[])
}

fn build(app: &adw::Application, args: &Args) -> Result<(), String> {
    if let Some(settings) = gtk::Settings::default() {
        // libadwaita ignores this legacy setting but GTK still applies it.
        #[allow(deprecated)]
        settings.set_gtk_application_prefer_dark_theme(false);
        // The harness hits the ends of the buffer constantly; GTK would ring
        // the desktop's error bell every time.
        settings.set_gtk_error_bell(false);
    }
    adw::StyleManager::default().set_color_scheme(args.scheme);
    let path = match &args.file {
        Some(path) => path.clone(),
        None => harness::write_sample(
            &std::env::temp_dir().join("igneous-live-preview-spike"),
            args.sample_kib,
        )
        .map_err(|e| e.to_string())?,
    };
    let (file, _) =
        igneous_core::fs::read_text(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let note_dir = path.parent().map(PathBuf::from).unwrap_or_default();
    let controller = Controller::new(file.text(), note_dir, args.concealment);
    if args.baseline {
        controller.set_baseline();
    } else if args.mode != Mode::Live {
        controller.set_mode(args.mode);
    }

    let modes = adw::ToggleGroup::new();
    for (name, label) in [
        ("live", "Live"),
        ("source", "Source"),
        ("reading", "Reading"),
    ] {
        modes.add(adw::Toggle::builder().name(name).label(label).build());
    }
    modes.set_active_name(Some(match args.mode {
        Mode::Live => "live",
        Mode::Source => "source",
        Mode::Reading => "reading",
    }));
    let c = controller.clone();
    modes.connect_active_name_notify(move |group| {
        c.set_mode(match group.active_name().as_deref() {
            Some("source") => Mode::Source,
            Some("reading") => Mode::Reading,
            _ => Mode::Live,
        });
    });

    let title = adw::WindowTitle::new(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        &format!("{:?} concealment", args.concealment),
    );
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    header.pack_end(&modes);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&controller.view)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(920)
        .default_height(1080)
        .content(&toolbar)
        .build();
    window.present();
    controller.view.grab_focus();
    harness::start(args.task.clone(), controller, &window, app);
    Ok(())
}
