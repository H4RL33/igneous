//! The linter in the app: settings from `.igneous/lint.json`, linting the
//! open note, a folder or the whole vault, and the problems shown in the
//! editor.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use igneous_core::VaultPath;
use igneous_core::fs::{self as corefs, Expect};
use igneous_core::settings::{self as vault_settings, LintSettings};
use igneous_lint::Linter;

use crate::note_page::{NotePage, State};
use crate::window::Window;

/// The key in `lint.json` turning on underlines for problems.
const SHOW_PROBLEMS: &str = "showProblems";

pub struct LintConfig {
    igneous_dir: PathBuf,
    settings: RefCell<LintSettings>,
    /// Set when `lint.json` couldn't be read: defaults are used, and the file
    /// is never overwritten.
    error: RefCell<Option<String>>,
    linter: RefCell<Rc<Linter>>,
    /// Pending recheck of the note being edited.
    problem_timer: RefCell<Option<glib::SourceId>>,
}

impl LintConfig {
    pub fn load(igneous_dir: &Path) -> Rc<Self> {
        let (settings, error) = match vault_settings::load::<LintSettings>(igneous_dir) {
            Ok(settings) => (settings, None),
            Err(e) => (LintSettings::default(), Some(e.to_string())),
        };
        Rc::new(Self {
            igneous_dir: igneous_dir.to_path_buf(),
            linter: RefCell::new(Rc::new(Linter::new(&settings))),
            settings: RefCell::new(settings),
            error: RefCell::new(error),
            problem_timer: RefCell::default(),
        })
    }

    pub fn settings(&self) -> LintSettings {
        self.settings.borrow().clone()
    }

    pub fn error(&self) -> Option<String> {
        self.error.borrow().clone()
    }

    pub fn linter(&self) -> Rc<Linter> {
        self.linter.borrow().clone()
    }

    /// Saves `settings` to `lint.json` and uses them from now on.
    pub fn set_settings(&self, settings: LintSettings) -> Result<(), String> {
        if let Some(error) = self.error() {
            return Err(format!(
                "lint.json can't be read, so it wasn't changed: {error}"
            ));
        }
        vault_settings::save(&self.igneous_dir, &settings).map_err(|e| e.to_string())?;
        self.linter.replace(Rc::new(Linter::new(&settings)));
        self.settings.replace(settings);
        Ok(())
    }

    /// Whether the editor underlines what the rules would change. Invalid
    /// YAML is underlined either way.
    pub fn show_problems(&self) -> bool {
        show_problems(&self.settings.borrow())
    }
}

pub fn show_problems(settings: &LintSettings) -> bool {
    settings
        .extra
        .get(SHOW_PROBLEMS)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn set_show_problems(settings: &mut LintSettings, show: bool) {
    settings
        .extra
        .insert(SHOW_PROBLEMS.to_owned(), serde_json::Value::Bool(show));
}

/// The problems to underline in a note.
pub fn problems(
    linter: &Linter,
    text: &str,
    path: &VaultPath,
    all: bool,
) -> Vec<igneous_editor::Diagnostic> {
    linter
        .lint(text, path)
        .diagnostics
        .into_iter()
        .filter(|d| all || d.rule == "yaml")
        .map(|d| {
            let explanation = igneous_lint::rules::get(d.rule)
                .map(|rule| rule.description().to_owned())
                .unwrap_or_default();
            igneous_editor::Diagnostic {
                range: d.range,
                message: d.message,
                explanation,
                rule: d.rule.to_owned(),
            }
        })
        .collect()
}

/// How a run over many files went.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub changed: usize,
    pub unchanged: usize,
    /// Ignored by the linter's settings, or open with unsaved edits.
    pub skipped: usize,
    pub failed: Vec<(VaultPath, String)>,
}

impl Report {
    pub fn describe(&self) -> String {
        let notes = |n: usize| {
            if n == 1 {
                "1 note".to_owned()
            } else {
                format!("{n} notes")
            }
        };
        let mut text = match self.changed {
            0 => "No notes needed changes".to_owned(),
            n => format!("Linted {}", notes(n)),
        };
        if self.skipped > 0 {
            text.push_str(&format!("; skipped {}", notes(self.skipped)));
        }
        if !self.failed.is_empty() {
            text.push_str(&format!(
                "; {} couldn’t be changed",
                notes(self.failed.len())
            ));
        }
        text
    }
}

/// Lints `paths` on disk (blocking: run it off the main thread). Files in
/// `skip` (open with unsaved edits) are left alone; every write checks the
/// file hasn't changed since it was read.
pub fn lint_files(
    settings: &LintSettings,
    root: &Path,
    paths: &[VaultPath],
    skip: &[VaultPath],
) -> Report {
    let linter = Linter::new(settings);
    let mut report = Report::default();
    for path in paths {
        if skip.contains(path) || linter.is_ignored(path) {
            report.skipped += 1;
            continue;
        }
        let abs = path.to_fs(root);
        let (mut file, stamp) = match corefs::read_text(&abs) {
            Ok(read) => read,
            Err(e) => {
                report.failed.push((path.clone(), e.to_string()));
                continue;
            }
        };
        let result = linter.lint(file.text(), path);
        if result.skipped {
            report.skipped += 1;
            continue;
        }
        if !result.changed() {
            report.unchanged += 1;
            continue;
        }
        file.set_text(result.text);
        match corefs::write_atomic(&abs, &file, Expect::Contents(&stamp)) {
            Ok(_) => report.changed += 1,
            Err(e) => report.failed.push((path.clone(), e.to_string())),
        }
    }
    report
}

// --- in the window ----------------------------------------------------------------

/// How a note's lint run went, for a toast.
fn describe_fixes(result: &igneous_lint::LintResult) -> String {
    if result.skipped {
        return "This note is ignored by the linter".to_owned();
    }
    let fixed = result
        .diagnostics
        .iter()
        .filter(|d| d.rule != "yaml")
        .count();
    let mut text = match fixed {
        0 => NO_CHANGES.to_owned(),
        1 => "Fixed 1 problem".to_owned(),
        n => format!("Fixed {n} problems"),
    };
    if result.diagnostics.iter().any(|d| d.rule == "yaml") {
        text.push_str("; the frontmatter isn’t valid YAML");
    }
    text
}

const NO_CHANGES: &str = "No changes";

impl Window {
    /// Lints a note as one undoable edit. Returns what happened, for a
    /// toast, or `None` for a tab without a file.
    pub fn lint_note(&self, note: &NotePage) -> Option<String> {
        let path = note.path()?;
        if note.state() == State::ReadOnly {
            return Some("This note is read-only".to_owned());
        }
        let text = note.text();
        let result = self.lint().linter().lint(&text, &path);
        if result.changed() {
            note.replace_text(&text, &result.text);
        }
        Some(describe_fixes(&result))
    }

    /// Ctrl+Alt+L.
    pub fn lint_selected_note(&self) {
        if let Some(note) = self.selected_note()
            && let Some(message) = self.lint_note(&note)
        {
            self.toast(&message);
        }
    }

    /// Ctrl+S: lints first if the vault asks for it, then saves. Autosave
    /// never lints: it would rewrite text while it's being typed.
    pub fn save_selected_note(&self) {
        let Some(note) = self.selected_note() else {
            return;
        };
        if self.lint().settings().lint_on_save
            && let Some(message) = self.lint_note(&note)
            && message != NO_CHANGES
        {
            self.toast(&message);
        }
        note.flush();
    }

    /// Asks before linting every note in `folder` (`None`: the whole vault).
    pub fn lint_folder_dialog(&self, folder: Option<VaultPath>) {
        let paths: Vec<VaultPath> = self
            .ctx()
            .files()
            .into_iter()
            .filter(|p| p.extension() == Some("md"))
            .filter(|p| folder.as_ref().is_none_or(|f| p.starts_with(f)))
            .collect();
        if paths.is_empty() {
            self.toast("There are no notes to lint here");
            return;
        }
        let count = if paths.len() == 1 {
            "1 Note".to_owned()
        } else {
            format!("{} Notes", paths.len())
        };
        let scope = match &folder {
            Some(folder) => format!("“{folder}”"),
            None => "this vault".to_owned(),
        };
        let mut body = format!(
            "Every note in {scope} is rewritten to follow this vault’s lint rules. \
             Notes with unsaved changes are left alone."
        );
        if self.sync().is_available() {
            body.push_str(
                "\n\nConsider committing first, so the changes are easy to review or undo.",
            );
        }
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Lint {count}?"))
            .body(body)
            .close_response("cancel")
            .default_response("lint")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("lint", "_Lint")]);
        dialog.set_response_appearance("lint", adw::ResponseAppearance::Suggested);
        let window = self.downgrade();
        dialog.connect_response(Some("lint"), move |_, _| {
            if let Some(window) = window.upgrade() {
                window.lint_paths(paths.clone());
            }
        });
        dialog.present(Some(self));
    }

    /// Lints `paths` on disk off the main thread, leaving notes with unsaved
    /// edits alone.
    pub fn lint_paths(&self, paths: Vec<VaultPath>) {
        let skip: Vec<VaultPath> = self
            .notes()
            .iter()
            .filter(|n| n.state() != State::Clean)
            .filter_map(NotePage::path)
            .collect();
        let settings = self.lint().settings();
        let root = self.root();
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let report =
                gio::spawn_blocking(move || lint_files(&settings, &root, &paths, &skip)).await;
            let Some(window) = window.upgrade() else {
                return;
            };
            match report {
                Ok(report) => {
                    for (path, error) in &report.failed {
                        tracing::warn!(%path, %error, "couldn't lint");
                    }
                    window.toast(&report.describe());
                }
                Err(_) => window.toast("Linting stopped unexpectedly"),
            }
        });
    }

    /// Rechecks `note` for problems a moment after it stops changing.
    pub fn schedule_problems(&self, note: &NotePage) {
        let lint = self.lint();
        if let Some(id) = lint.problem_timer.take() {
            id.remove();
        }
        let window = self.downgrade();
        let note = note.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(600), move || {
            let (Some(window), Some(note)) = (window.upgrade(), note.upgrade()) else {
                return;
            };
            window.lint().problem_timer.take();
            window.check_problems(&note);
        });
        lint.problem_timer.replace(Some(id));
    }

    /// Lints `note`'s text in the background and underlines the problems,
    /// unless the text has changed meanwhile.
    pub fn check_problems(&self, note: &NotePage) {
        let Some(path) = note.path() else { return };
        let text = note.text();
        let settings = self.lint().settings();
        let all = show_problems(&settings);
        // Without underlines only invalid frontmatter is reported.
        if !all && !text.starts_with("---") {
            note.view().set_diagnostics(Vec::new());
            return;
        }
        let note = note.downgrade();
        glib::spawn_future_local(async move {
            let checked = text.clone();
            let found = gio::spawn_blocking(move || {
                problems(&Linter::new(&settings), &checked, &path, all)
            })
            .await;
            if let (Ok(found), Some(note)) = (found, note.upgrade())
                && note.text() == text
            {
                note.view().set_diagnostics(found);
            }
        });
    }

    /// Rechecks every open note (after the settings change).
    pub fn refresh_problems(&self) {
        for note in self.notes() {
            self.check_problems(&note);
        }
    }

    /// The Fix button on an underline: what `rule` alone would change.
    pub fn fix_rule(&self, note: &NotePage, rule: &str) {
        let Some(path) = note.path() else { return };
        let text = note.text();
        let edits = self.lint().linter().fix_rule(rule, &text, &path);
        if edits.is_empty() {
            return;
        }
        let fixed = igneous_markdown::edit::apply(&text, &edits);
        note.replace_text(&text, &fixed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lints_files_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let abs = dir.path().join(name);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(abs, text).unwrap();
        };
        write("a.md", "trailing   \n");
        write("b.md", "fine\n");
        write("c.md", "dirty  \n");
        write("Ignored/d.md", "spaces  \n");
        let mut settings = igneous_lint::recommended();
        settings.ignore_folders = vec!["Ignored".into()];
        let p = |s: &str| VaultPath::new(s).unwrap();
        let report = lint_files(
            &settings,
            dir.path(),
            &[p("a.md"), p("b.md"), p("c.md"), p("Ignored/d.md")],
            &[p("c.md")],
        );
        assert_eq!(
            report,
            Report {
                changed: 1,
                unchanged: 1,
                skipped: 2,
                failed: vec![],
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.md")).unwrap(),
            "trailing\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("c.md")).unwrap(),
            "dirty  \n"
        );
        assert_eq!(report.describe(), "Linted 1 note; skipped 2 notes");
    }

    #[test]
    fn problems_setting_round_trips() {
        let mut settings = LintSettings::default();
        assert!(!show_problems(&settings));
        set_show_problems(&mut settings, true);
        let bytes = vault_settings::to_bytes(&settings);
        let back: LintSettings = vault_settings::from_bytes(&bytes).unwrap();
        assert!(show_problems(&back));
    }

    #[test]
    fn invalid_yaml_is_always_a_problem() {
        let linter = Linter::new(&LintSettings {
            rules: Default::default(),
            ..LintSettings::default()
        });
        let p = VaultPath::new("a.md").unwrap();
        let found = problems(&linter, "---\na: [b\n---\ntext  \n", &p, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, "yaml");
    }
}
