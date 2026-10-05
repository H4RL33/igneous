//! Templates: notes in the templates folder, inserted at the cursor with
//! `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}` and
//! `{{time:FORMAT}}` filled in. A template's properties merge into the
//! note's (lists are combined; keys the note already has are kept) instead
//! of adding a second frontmatter block.

use adw::prelude::*;
use gtk::glib;
use igneous_core::VaultPath;
use igneous_core::datefmt;
use igneous_core::settings::TemplateSettings;
use igneous_markdown::TextEdit;
use igneous_markdown::frontmatter::{self, Value};
use jiff::Zoned;

use crate::note_page::NotePage;
use crate::window::Window;

/// Fills in a template's placeholders. Unknown ones are left as written.
pub fn render(template: &str, title: &str, now: &Zoned, settings: &TemplateSettings) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = after[..close].trim();
        let (name, format) = match inner.split_once(':') {
            Some((name, format)) => (name.trim(), Some(format.trim())),
            None => (inner, None),
        };
        match (name, format) {
            ("title", None) => out.push_str(title),
            ("date", format) => out.push_str(&datefmt::format(
                now,
                format.unwrap_or(&settings.date_format),
            )),
            ("time", format) => out.push_str(&datefmt::format(
                now,
                format.unwrap_or(&settings.time_format),
            )),
            _ => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// The edits that insert `template` (already rendered) into `note` with
/// the cursor at byte `cursor`: one replacing the note's frontmatter with
/// the merged properties (if they changed), and one inserting the body.
/// Returns the edits and where the inserted body ends.
pub fn insert(note: &str, template: &str, cursor: usize) -> (Vec<TextEdit>, usize) {
    let (properties, body) = match frontmatter::parse(template) {
        Some(fm) if fm.error.is_none() => (fm.entries, &template[fm.range.end..]),
        _ => (Vec::new(), template),
    };
    let old_end = frontmatter::detect(note).map_or(0, |(range, _)| range.end);
    let mut merged = note.to_owned();
    for entry in properties {
        let existing = frontmatter::parse(&merged)
            .and_then(|fm| fm.entries.into_iter().find(|e| e.key == entry.key));
        let value = match (existing, &entry.value) {
            // Lists combine, without repeats.
            (Some(old), Value::List(new)) if matches!(old.value, Value::List(_)) => {
                let Value::List(mut items) = old.value else {
                    unreachable!()
                };
                for item in new {
                    if !items.contains(item) {
                        items.push(item.clone());
                    }
                }
                Value::List(items)
            }
            // The note's own value wins.
            (Some(_), _) => continue,
            (None, value) => value.clone(),
        };
        if let Ok(edits) = frontmatter::set(&merged, &entry.key, &value) {
            merged = igneous_markdown::edit::apply(&merged, &edits);
        }
    }
    let new_end = frontmatter::detect(&merged).map_or(0, |(range, _)| range.end);
    let at = cursor.clamp(old_end, note.len());
    let mut edits = Vec::new();
    if note[..old_end] != merged[..new_end] {
        edits.push(TextEdit::replace(0..old_end, &merged[..new_end]));
    }
    edits.push(TextEdit::insert(at, body));
    let shift = new_end as isize - old_end as isize;
    let end = (at + body.len()) as isize + shift;
    (edits, end.max(0) as usize)
}

/// Applies byte-offset edits to a note's buffer as one undoable step.
pub fn apply_to_note(note: &NotePage, edits: &[TextEdit]) {
    let text = note.text();
    let chars = |byte: usize| text[..byte].chars().count() as i32;
    // Last first, so earlier offsets stay valid; at the same offset the later
    // edit goes in first and ends up after the earlier one.
    let mut sorted: Vec<(usize, &TextEdit)> = edits.iter().enumerate().collect();
    sorted.sort_by_key(|(i, e)| std::cmp::Reverse((e.range.start, *i)));
    let buffer = note.buffer();
    buffer.begin_user_action();
    for (_, edit) in sorted {
        let mut start = buffer.iter_at_offset(chars(edit.range.start));
        let mut end = buffer.iter_at_offset(chars(edit.range.end));
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &edit.insert);
    }
    buffer.end_user_action();
}

impl Window {
    /// The notes in the templates folder, sorted.
    pub fn templates(&self) -> Vec<VaultPath> {
        let Some(folder) = self.ctx().settings.borrow().templates.folder.clone() else {
            return Vec::new();
        };
        let Ok(folder) = VaultPath::new(folder.trim_matches('/')) else {
            return Vec::new();
        };
        let mut found: Vec<VaultPath> = self
            .ctx()
            .files()
            .into_iter()
            .filter(|p| p.starts_with(&folder) && p.extension() == Some("md"))
            .collect();
        found.sort_by(|a, b| igneous_core::path::natural_cmp(a.as_str(), b.as_str()));
        found
    }

    /// Inserts the template at `template` into the open note.
    pub fn insert_template(&self, template: &VaultPath) {
        let Some(note) = self.selected_note() else {
            self.toast("Open a note to insert a template into");
            return;
        };
        let source = match igneous_core::fs::read_text(&self.ctx().abs(template)) {
            Ok((file, _)) => file.text().to_owned(),
            Err(e) => {
                self.toast(&format!("Couldn’t read the template: {e}"));
                return;
            }
        };
        let title = note.path().map(|p| p.stem().to_owned()).unwrap_or_default();
        let settings = self.ctx().settings.borrow().templates.clone();
        let rendered = render(&source, &title, &Zoned::now(), &settings);
        let (edits, end) = insert(&note.text(), &rendered, note.cursor_byte());
        apply_to_note(&note, &edits);
        note.set_cursor_byte(end);
        note.focus_editor();
    }

    /// A list of templates to pick from.
    pub fn show_template_picker(&self) {
        if self.selected_note().is_none() {
            self.toast("Open a note to insert a template into");
            return;
        }
        if self.ctx().settings.borrow().templates.folder.is_none() {
            self.toast("Choose a templates folder in Preferences → Plugins");
            return;
        }
        let templates = self.templates();
        if templates.is_empty() {
            self.toast("The templates folder has no notes yet");
            return;
        }
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        let dialog = adw::Dialog::builder()
            .title("Insert Template")
            .content_width(420)
            .content_height(420)
            .build();
        for template in templates {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(template.stem()))
                .subtitle(glib::markup_escape_text(template.as_str()))
                .activatable(true)
                .build();
            let window = self.downgrade();
            let dialog_weak = dialog.downgrade();
            row.connect_activated(move |_| {
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.close();
                }
                if let Some(window) = window.upgrade() {
                    window.insert_template(&template);
                }
            });
            list.append(&row);
        }
        let clamp = adw::Clamp::builder()
            .child(&list)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(12)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&clamp)
                .build(),
        ));
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(self));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        "2026-10-05T09:30:00[Europe/London]".parse().unwrap()
    }

    #[test]
    fn placeholders() {
        let settings = TemplateSettings::default();
        assert_eq!(
            render(
                "# {{title}}\n{{date}} {{time}} {{date:dddd D MMMM}} {{time:HH}} {{unknown}}",
                "Plans",
                &now(),
                &settings
            ),
            "# Plans\n2026-10-05 09:30 Monday 5 October 09 {{unknown}}"
        );
        assert_eq!(render("{{ title }} {{", "x", &now(), &settings), "x {{");
    }

    #[test]
    fn properties_merge() {
        let note = "---\ntags:\n  - a\nstatus: done\n---\nbody\n";
        let template = "---\ntags:\n  - b\nstatus: draft\nkind: idea\n---\nInserted\n";
        let (edits, end) = insert(note, template, note.len());
        let out = igneous_markdown::edit::apply(note, &edits);
        assert!(out.contains("  - a\n  - b\n"), "{out}");
        assert!(out.contains("status: done"), "{out}");
        assert!(out.contains("kind: idea"), "{out}");
        assert!(out.ends_with("body\nInserted\n"), "{out}");
        assert_eq!(&out[..end], out.as_str());
        // A note without properties gets the template's.
        let (edits, _) = insert("plain\n", template, 0);
        let out = igneous_markdown::edit::apply("plain\n", &edits);
        assert!(out.starts_with("---\n"), "{out}");
        assert!(out.contains("Inserted\nplain\n"), "{out}");
    }
}
