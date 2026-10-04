//! The sidebar's Tags pane: every tag as a nested tree with counts. Clicking
//! a tag searches for it; its menu renames it across the vault.

use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::window::Window;

/// A tag and the tags nested under it.
#[derive(Debug, Default, PartialEq)]
pub struct TagNode {
    /// Notes using this tag itself.
    pub count: usize,
    pub children: BTreeMap<String, TagNode>,
}

impl TagNode {
    /// Notes using this tag or any nested one (an upper bound: a note may
    /// use several).
    pub fn total(&self) -> usize {
        self.count + self.children.values().map(TagNode::total).sum::<usize>()
    }
}

/// Builds the tree from `a/b`-style tags and their note counts.
pub fn tree(tags: &[(String, usize)]) -> TagNode {
    let mut root = TagNode::default();
    for (tag, count) in tags {
        let mut node = &mut root;
        for part in tag.split('/').filter(|p| !p.is_empty()) {
            node = node.children.entry(part.to_owned()).or_default();
        }
        node.count += count;
    }
    root
}

pub struct TagsPane {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    window: glib::WeakRef<Window>,
    /// Tags whose children are shown.
    expanded: std::cell::RefCell<std::collections::HashSet<String>>,
    shown: std::cell::RefCell<Vec<(String, usize)>>,
}

impl TagsPane {
    pub fn new(window: &Window) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        list.connect_row_activated(|_, row| row.emit_activate());
        Rc::new(Self {
            widget,
            list,
            window: window.downgrade(),
            expanded: Default::default(),
            shown: Default::default(),
        })
    }

    pub fn set_tags(self: &Rc<Self>, tags: &[(String, usize)]) {
        if self.shown.borrow().as_slice() == tags {
            return;
        }
        self.shown.replace(tags.to_vec());
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        self.list.remove_all();
        let tags = self.shown.borrow().clone();
        if tags.is_empty() {
            let page = adw::StatusPage::builder()
                .icon_name("tag-symbolic")
                .title("No Tags")
                .description("Tags written as #tag or in a note’s tags property appear here")
                .css_classes(["compact"])
                .build();
            let row = gtk::ListBoxRow::builder()
                .child(&page)
                .activatable(false)
                .build();
            self.list.append(&row);
            return;
        }
        self.add_children(&tree(&tags), "", 0);
    }

    fn add_children(self: &Rc<Self>, node: &TagNode, prefix: &str, depth: i32) {
        // Most-used first, like Obsidian.
        let mut children: Vec<(&String, &TagNode)> = node.children.iter().collect();
        children.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));
        for (name, child) in children {
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let expanded = self.expanded.borrow().contains(&full);
            self.list
                .append(&self.row(&full, name, child, depth, expanded));
            if expanded {
                self.add_children(child, &full, depth + 1);
            }
        }
    }

    fn row(
        self: &Rc<Self>,
        full: &str,
        name: &str,
        node: &TagNode,
        depth: i32,
        expanded: bool,
    ) -> gtk::ListBoxRow {
        let content = gtk::Box::builder()
            .spacing(4)
            .margin_start(depth * 16)
            .build();
        if node.children.is_empty() {
            content.append(&gtk::Box::builder().width_request(24).build());
        } else {
            let expander = gtk::Button::builder()
                .icon_name(if expanded {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                })
                .css_classes(["flat", "circular"])
                .tooltip_text(if expanded { "Collapse" } else { "Expand" })
                .build();
            let weak = Rc::downgrade(self);
            let tag = full.to_owned();
            expander.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    let mut expanded = pane.expanded.borrow_mut();
                    if !expanded.remove(&tag) {
                        expanded.insert(tag.clone());
                    }
                    drop(expanded);
                    pane.rebuild();
                }
            });
            content.append(&expander);
        }
        content.append(
            &gtk::Label::builder()
                .label(format!("#{name}"))
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        content.append(
            &gtk::Label::builder()
                .label(node.total().to_string())
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        let rename = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Rename Tag")
            .css_classes(["flat", "circular"])
            .build();
        let window = self.window.clone();
        let tag = full.to_owned();
        rename.connect_clicked(move |_| {
            if let Some(window) = window.upgrade() {
                window.rename_tag_dialog(&tag);
            }
        });
        content.append(&rename);
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .tooltip_text(format!("#{full}"))
            .build();
        let window = self.window.clone();
        let tag = full.to_owned();
        row.connect_activate(move |_| {
            if let Some(window) = window.upgrade() {
                window.search_vault(&format!("tag:#{tag}"));
            }
        });
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nests_tags() {
        let root = tree(&[
            ("project".into(), 2),
            ("project/igneous".into(), 3),
            ("project/igneous/ui".into(), 1),
            ("idea".into(), 1),
        ]);
        assert_eq!(root.children.len(), 2);
        let project = &root.children["project"];
        assert_eq!(project.count, 2);
        assert_eq!(project.total(), 6);
        assert_eq!(project.children["igneous"].total(), 4);
    }
}
