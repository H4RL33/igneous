//! The inspector, beside the note: what links here (linked and unlinked
//! mentions), what the note links to, its outline, and its local graph.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use igneous_core::VaultPath;
use igneous_index::{LinkHit, Mention, OutLink};
use igneous_markdown::Heading;

use crate::local_graph::LocalGraph;
use crate::window::Window;

pub struct Inspector {
    pub widget: adw::ToolbarView,
    pub stack: adw::ViewStack,
    backlinks: gtk::Box,
    outgoing: gtk::Box,
    outline: gtk::Box,
    pub local_graph: Rc<LocalGraph>,
    window: glib::WeakRef<Window>,
}

/// What the backlinks view shows for one note.
pub struct Links {
    pub path: VaultPath,
    pub backlinks: Vec<LinkHit>,
    pub mentions: Vec<Mention>,
    pub outgoing: Vec<OutLink>,
    /// The note's title and aliases, to highlight in unlinked mentions.
    pub names: Vec<String>,
}

fn page(stack: &adw::ViewStack, name: &str, title: &str, icon: &str) -> gtk::Box {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_bottom(12)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();
    stack.add_titled_with_icon(&scrolled, Some(name), title, icon);
    content
}

impl Inspector {
    pub fn new(window: &Window) -> Rc<Self> {
        let stack = adw::ViewStack::new();
        let backlinks = page(
            &stack,
            "backlinks",
            "Backlinks",
            "mail-reply-sender-symbolic",
        );
        let outgoing = page(&stack, "outgoing", "Outgoing", "mail-forward-symbolic");
        let outline = page(&stack, "outline", "Outline", "view-list-bullet-symbolic");
        let weak = window.downgrade();
        let local_graph = LocalGraph::new(move |path, new_tab| {
            if let Some(window) = weak.upgrade() {
                window.open_path(path, new_tab);
            }
        });
        stack.add_titled_with_icon(
            &local_graph.widget,
            Some("graph"),
            "Graph",
            "network-workgroup-symbolic",
        );
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&stack)
            .display_mode(adw::InlineViewSwitcherDisplayMode::Icons)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(6)
            .css_classes(["flat"])
            .build();
        let header = adw::HeaderBar::builder()
            .show_title(false)
            .show_start_title_buttons(false)
            .build();
        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.add_top_bar(&switcher);
        widget.set_content(Some(&stack));
        Rc::new(Self {
            widget,
            stack,
            backlinks,
            outgoing,
            outline,
            local_graph,
            window: window.downgrade(),
        })
    }

    fn clear(container: &gtk::Box) {
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
    }

    fn heading(container: &gtk::Box, text: &str) {
        container.append(
            &gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .margin_start(12)
                .margin_top(12)
                .margin_bottom(6)
                .css_classes(["heading"])
                .build(),
        );
    }

    fn empty(container: &gtk::Box, text: &str) {
        container.append(
            &gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .wrap(true)
                .margin_start(12)
                .margin_end(12)
                .css_classes(["dim-label"])
                .build(),
        );
    }

    fn list() -> gtk::ListBox {
        gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::None)
            .build()
    }

    /// A row for one line of another note, opening it there when activated.
    fn context_row(
        &self,
        source: &VaultPath,
        line_text: &str,
        highlight: &[String],
        at: usize,
    ) -> gtk::ListBoxRow {
        let title = gtk::Label::builder()
            .label(crate::files::display_name(source, false).0)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption-heading"])
            .build();
        let context = gtk::Label::builder()
            .use_markup(true)
            .label(highlighted(line_text.trim(), highlight))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        content.append(&title);
        content.append(&context);
        let row = gtk::ListBoxRow::builder().child(&content).build();
        row.set_tooltip_text(Some(source.as_str()));
        let window = self.window.clone();
        let source = source.clone();
        row.connect_activate(move |_| {
            if let Some(window) = window.upgrade() {
                window.open_at(&source, at);
            }
        });
        row
    }

    pub fn show_links(self: &Rc<Self>, links: Option<Links>) {
        Self::clear(&self.backlinks);
        Self::clear(&self.outgoing);
        let Some(links) = links else {
            Self::empty(&self.backlinks, "Open a note to see what links to it.");
            Self::empty(&self.outgoing, "Open a note to see what it links to.");
            return;
        };

        Self::heading(
            &self.backlinks,
            &format!("Linked Mentions ({})", links.backlinks.len()),
        );
        if links.backlinks.is_empty() {
            Self::empty(&self.backlinks, "No notes link here yet.");
        } else {
            let list = Self::list();
            for hit in &links.backlinks {
                list.append(&self.context_row(&hit.source, &hit.line_text, &[], hit.range.start));
            }
            list.connect_row_activated(|_, row| row.emit_activate());
            self.backlinks.append(&list);
        }

        Self::heading(
            &self.backlinks,
            &format!("Unlinked Mentions ({})", links.mentions.len()),
        );
        if links.mentions.is_empty() {
            Self::empty(&self.backlinks, "No other notes mention this one by name.");
        } else {
            let list = Self::list();
            for mention in &links.mentions {
                let row = self.context_row(
                    &mention.source,
                    &mention.line_text,
                    &links.names,
                    mention.range.start,
                );
                // A button turning the mention into a link.
                let link = gtk::Button::builder()
                    .label("Link")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .tooltip_text("Turn this mention into a link")
                    .build();
                let content = row.child().and_downcast::<gtk::Box>().unwrap();
                let line = gtk::Box::builder().spacing(6).build();
                row.set_child(Some(&line));
                content.set_hexpand(true);
                line.append(&content);
                line.append(&link);
                let window = self.window.clone();
                let mention = mention.clone();
                let target = links.path.clone();
                link.connect_clicked(move |_| {
                    if let Some(window) = window.upgrade() {
                        window.link_mention(&mention, &target);
                    }
                });
                list.append(&row);
            }
            list.connect_row_activated(|_, row| row.emit_activate());
            self.backlinks.append(&list);
        }

        let links_out: Vec<&OutLink> = links.outgoing.iter().filter(|l| !l.external).collect();
        Self::heading(&self.outgoing, &format!("Links ({})", links_out.len()));
        if links_out.is_empty() {
            Self::empty(&self.outgoing, "This note doesn’t link to anything.");
        } else {
            let list = Self::list();
            let mut seen = std::collections::HashSet::new();
            for link in links_out {
                let key = link
                    .resolved
                    .as_ref()
                    .map_or_else(|| link.reference.target.to_lowercase(), |p| p.to_string());
                if !seen.insert(key) {
                    continue;
                }
                let (title, subtitle) = match &link.resolved {
                    Some(path) => (
                        crate::files::display_name(path, false).0,
                        path.parent().map(|p| p.to_string()).unwrap_or_default(),
                    ),
                    None => (link.reference.target.clone(), "Not created yet".to_owned()),
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&title))
                    .subtitle(glib::markup_escape_text(&subtitle))
                    .activatable(true)
                    .build();
                if link.resolved.is_none() {
                    row.add_css_class("dim-label");
                }
                if link.embed {
                    row.add_prefix(&gtk::Image::from_icon_name("insert-image-symbolic"));
                }
                let window = self.window.clone();
                let reference = link.reference.clone();
                let from = links.path.clone();
                row.connect_activated(move |_| {
                    if let Some(window) = window.upgrade() {
                        window.follow_link(&reference, Some(&from), false);
                    }
                });
                list.append(&row);
            }
            self.outgoing.append(&list);
        }
    }

    pub fn show_outline(self: &Rc<Self>, path: Option<&VaultPath>, headings: &[Heading]) {
        Self::clear(&self.outline);
        Self::heading(&self.outline, "Outline");
        if headings.is_empty() {
            Self::empty(
                &self.outline,
                if path.is_some() {
                    "This note has no headings."
                } else {
                    "Open a note to see its outline."
                },
            );
            return;
        }
        let list = Self::list();
        let top = headings.iter().map(|h| h.level).min().unwrap_or(1);
        for heading in headings {
            let label = gtk::Label::builder()
                .label(&heading.text)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .margin_start(i32::from(heading.level - top) * 14)
                .build();
            if heading.level == top {
                label.add_css_class("heading");
            }
            let row = gtk::ListBoxRow::builder().child(&label).build();
            let window = self.window.clone();
            let at = heading.range.start;
            row.connect_activate(move |_| {
                if let Some(note) = window.upgrade().and_then(|w| w.selected_note()) {
                    note.set_cursor_byte(at);
                    note.focus_editor();
                }
            });
            list.append(&row);
        }
        list.connect_row_activated(|_, row| row.emit_activate());
        self.outline.append(&list);
    }
}

/// `text` as markup, with whole-word, case-insensitive matches of `names`
/// in bold.
fn highlighted(text: &str, names: &[String]) -> String {
    let lower = text.to_lowercase();
    let mut marks = vec![false; text.len()];
    for name in names.iter().filter(|n| !n.is_empty()) {
        let name = name.to_lowercase();
        let mut from = 0;
        while let Some(i) = lower[from..].find(&name) {
            let start = from + i;
            let end = start + name.len();
            // Lower-casing can change lengths; skip anything misaligned.
            if end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end) {
                marks[start..end].fill(true);
            }
            from = end;
        }
    }
    let mut out = String::new();
    let mut bold = false;
    for (i, ch) in text.char_indices() {
        if marks[i] != bold {
            out.push_str(if marks[i] { "<b>" } else { "</b>" });
            bold = marks[i];
        }
        out.push_str(&glib::markup_escape_text(ch.encode_utf8(&mut [0; 4])));
    }
    if bold {
        out.push_str("</b>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_names() {
        assert_eq!(
            highlighted("See the roadmap & Roadmap", &["Roadmap".into()]),
            "See the <b>roadmap</b> &amp; <b>Roadmap</b>"
        );
        assert_eq!(highlighted("plain", &[]), "plain");
    }
}
