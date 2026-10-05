//! The sidebar's Search pane: Obsidian's search syntax over the whole vault,
//! with results grouped by note and matches highlighted in context.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use igneous_core::VaultPath;
use igneous_query::search::{Matcher, SearchOptions, snippets};

use crate::window::Window;

/// Most notes listed, and most lines shown per note.
const MAX_NOTES: usize = 200;
const MAX_LINES: usize = 8;

pub struct SearchPane {
    pub widget: gtk::Box,
    pub entry: gtk::SearchEntry,
    match_case: gtk::ToggleButton,
    summary: gtk::Label,
    results: gtk::Box,
    window: glib::WeakRef<Window>,
    /// Bumped by every search, so late results of an older one are dropped.
    generation: Cell<u64>,
    /// The notes in the last results, for tests.
    pub found: std::cell::RefCell<Vec<VaultPath>>,
    timer: std::cell::RefCell<Option<glib::SourceId>>,
}

/// One matching note, ready to show.
struct Found {
    path: VaultPath,
    /// Each matching line as markup, with the byte offset to open it at.
    lines: Vec<(String, usize)>,
    matches: usize,
}

impl SearchPane {
    pub fn new(window: &Window) -> Rc<Self> {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Search notes")
            .hexpand(true)
            .build();
        let match_case = gtk::ToggleButton::builder()
            .icon_name("format-text-plaintext-symbolic")
            .tooltip_text("Match Case")
            .css_classes(["flat"])
            .build();
        let row = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .build();
        row.append(&entry);
        row.append(&match_case);
        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .css_classes(["dim-label", "caption"])
            .build();
        let results = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_bottom(12)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&results)
            .build();
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        widget.append(&row);
        widget.append(&summary);
        widget.append(&scrolled);
        let pane = Rc::new(Self {
            widget,
            entry,
            match_case,
            summary,
            results,
            window: window.downgrade(),
            generation: Cell::default(),
            found: Default::default(),
            timer: Default::default(),
        });
        let weak = Rc::downgrade(&pane);
        pane.entry.connect_search_changed(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.schedule();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.match_case.connect_toggled(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.run();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.entry.connect_activate(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.open_first();
            }
        });
        pane.show_summary("Search the vault with Obsidian’s search syntax: terms, \"phrases\", OR, -exclusions, /regex/, file:, path:, tag:, line:, section:, task:, [property:value]");
        pane
    }

    /// Searches for `query`, as if typed.
    pub fn search(self: &Rc<Self>, query: &str) {
        self.entry.set_text(query);
        self.run();
    }

    fn show_summary(&self, text: &str) {
        self.summary.set_label(text);
    }

    fn schedule(self: &Rc<Self>) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            if let Some(pane) = weak.upgrade() {
                pane.timer.take();
                pane.run();
            }
        });
        self.timer.replace(Some(id));
    }

    /// Searches again, e.g. after the vault changed.
    pub fn refresh(self: &Rc<Self>) {
        if !self.entry.text().trim().is_empty() {
            self.schedule();
        }
    }

    fn run(self: &Rc<Self>) {
        let query = self.entry.text().trim().to_owned();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if query.is_empty() {
            Self::clear(&self.results);
            self.show_summary("");
            return;
        }
        let options = SearchOptions {
            match_case: self.match_case.is_active(),
        };
        if let Err(e) = Matcher::parse(&query, options) {
            Self::clear(&self.results);
            self.show_summary(&format!("Can’t search for that: {e}"));
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let index = window.index().clone();
        let pane = self.clone();
        glib::spawn_future_local(async move {
            let data = index.data().await;
            let found = gio::spawn_blocking(move || {
                let Ok(matcher) = Matcher::parse(&query, options) else {
                    return Vec::new();
                };
                let mut found = Vec::new();
                for note in data.iter() {
                    let Some(hit) = matcher.matches(note) else {
                        continue;
                    };
                    let lines = snippets(&note.text, &hit.content)
                        .into_iter()
                        .take(MAX_LINES)
                        .map(|s| (snippet_markup(&note.text, &s), s.range.start))
                        .collect();
                    found.push(Found {
                        path: note.path.clone(),
                        lines,
                        matches: hit.content.len().max(1),
                    });
                }
                found.sort_by(|a, b| {
                    igneous_core::path::natural_cmp(a.path.as_str(), b.path.as_str())
                });
                found
            })
            .await
            .unwrap_or_default();
            if pane.generation.get() == generation {
                pane.show(found);
            }
        });
    }

    fn clear(container: &gtk::Box) {
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
    }

    fn show(self: &Rc<Self>, found: Vec<Found>) {
        Self::clear(&self.results);
        self.found
            .replace(found.iter().map(|f| f.path.clone()).collect());
        let total: usize = found.iter().map(|f| f.matches).sum();
        self.show_summary(&match found.len() {
            0 => "No results".to_owned(),
            1 => format!("{total} {} in 1 note", plural(total, "result")),
            n => format!("{total} {} in {n} notes", plural(total, "result")),
        });
        for item in found.into_iter().take(MAX_NOTES) {
            let list = gtk::ListBox::builder()
                .css_classes(["navigation-sidebar"])
                .selection_mode(gtk::SelectionMode::None)
                .build();
            let (name, _) = crate::files::display_name(&item.path, false);
            let title = gtk::Label::builder()
                .label(&name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .hexpand(true)
                .css_classes(["heading"])
                .build();
            let count = gtk::Label::builder()
                .label(item.matches.to_string())
                .css_classes(["dim-label", "caption"])
                .build();
            let header = gtk::Box::builder().spacing(6).build();
            header.append(&title);
            header.append(&count);
            let header_row = gtk::ListBoxRow::builder()
                .child(&header)
                .tooltip_text(item.path.as_str())
                .build();
            let window = self.window.clone();
            let path = item.path.clone();
            crate::rows::on_activate(&header_row, move || {
                if let Some(window) = window.upgrade() {
                    window.open_path(&path, false);
                }
            });
            list.append(&header_row);
            for (markup, at) in item.lines {
                let label = gtk::Label::builder()
                    .use_markup(true)
                    .label(markup)
                    .xalign(0.0)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .margin_start(12)
                    .css_classes(["caption"])
                    .build();
                let row = gtk::ListBoxRow::builder().child(&label).build();
                let window = self.window.clone();
                let path = item.path.clone();
                crate::rows::on_activate(&row, move || {
                    if let Some(window) = window.upgrade() {
                        window.open_at(&path, at);
                    }
                });
                list.append(&row);
            }
            self.results.append(&list);
        }
    }

    /// Enter in the search entry opens the first result.
    fn open_first(&self) {
        if let Some(list) = self.results.first_child().and_downcast::<gtk::ListBox>()
            && let Some(row) = list.row_at_index(0)
        {
            row.emit_activate();
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// A matching line as markup, matches in bold, trimmed around the first
/// match if it's long.
fn snippet_markup(text: &str, snippet: &igneous_query::search::Snippet) -> String {
    const CONTEXT: usize = 60;
    let line = &text[snippet.range.clone()];
    let first = snippet
        .highlights
        .first()
        .map_or(0, |h| h.start.saturating_sub(snippet.range.start));
    let mut start = first.saturating_sub(CONTEXT);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut pos = start;
    for h in &snippet.highlights {
        let (a, b) = (
            h.start.saturating_sub(snippet.range.start).max(pos),
            (h.end - snippet.range.start).min(line.len()),
        );
        if a >= b || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
            continue;
        }
        out.push_str(&glib::markup_escape_text(&line[pos..a]));
        out.push_str("<b>");
        out.push_str(&glib::markup_escape_text(&line[a..b]));
        out.push_str("</b>");
        pos = b;
    }
    out.push_str(&glib::markup_escape_text(&line[pos..]));
    out
}
