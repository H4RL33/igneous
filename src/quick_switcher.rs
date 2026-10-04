//! The quick switcher (Ctrl+O): fuzzy-find a note by name, or create one.

use std::cell::RefCell;
use std::rc::Rc;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib, pango};
use igneous_core::VaultPath;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const LIMIT: usize = 200;

/// What the user picked.
pub enum Choice {
    Open { path: VaultPath, new_tab: bool },
    Create { name: String },
}

/// The text matched against: the path without `.md`.
fn haystack(path: &VaultPath) -> String {
    let s = path.as_str();
    s.strip_suffix(".md").unwrap_or(s).to_owned()
}

/// Orders `files` for `query`: best fuzzy matches first, favouring recently
/// opened files. With an empty query, recent files come first.
pub fn rank(
    query: &str,
    files: &[VaultPath],
    recent: &[VaultPath],
    limit: usize,
) -> Vec<VaultPath> {
    let recency = |path: &VaultPath| recent.iter().position(|r| r == path);
    let query = query.trim();
    if query.is_empty() {
        let mut out: Vec<VaultPath> = recent
            .iter()
            .filter(|r| files.contains(r))
            .cloned()
            .collect();
        let mut rest: Vec<&VaultPath> = files.iter().filter(|f| recency(f).is_none()).collect();
        rest.sort_by(|a, b| igneous_core::path::natural_cmp(a.as_str(), b.as_str()));
        out.extend(rest.into_iter().cloned());
        out.truncate(limit);
        return out;
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, &VaultPath)> = files
        .iter()
        .filter_map(|path| {
            let hay = haystack(path);
            let score = pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher)?;
            // Recently opened files get a modest boost.
            let boost = recency(path).map_or(0, |i| 40u32.saturating_sub(i as u32 * 4));
            Some((score + boost, path))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then(a.as_str().len().cmp(&b.as_str().len()))
            .then_with(|| igneous_core::path::natural_cmp(a.as_str(), b.as_str()))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, p)| p.clone())
        .collect()
}

type ChooseFn = dyn Fn(Choice);

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/quick-switcher.ui")]
    pub struct QuickSwitcher {
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub results_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,
        pub results: gtk::StringList,
        pub selection: RefCell<Option<gtk::SingleSelection>>,
        pub files: RefCell<Vec<VaultPath>>,
        pub recent: RefCell<Vec<VaultPath>>,
        pub on_choose: RefCell<Option<Rc<ChooseFn>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QuickSwitcher {
        const NAME: &'static str = "IgneousQuickSwitcher";
        type Type = super::QuickSwitcher;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for QuickSwitcher {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let selection = gtk::SingleSelection::new(Some(self.results.clone()));
            self.results_view.set_model(Some(&selection));
            self.results_view.set_factory(Some(&row_factory()));
            self.selection.replace(Some(selection));

            self.search_entry.connect_search_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update()
            ));
            self.results_view.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |_, position| obj.choose_at(position, false)
            ));
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, state| obj.on_key(key, state)
            ));
            self.search_entry.add_controller(keys);
            self.search_entry.set_key_capture_widget(Some(&*obj));
        }
    }

    impl WidgetImpl for QuickSwitcher {}
    impl AdwDialogImpl for QuickSwitcher {}
}

glib::wrapper! {
    pub struct QuickSwitcher(ObjectSubclass<imp::QuickSwitcher>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl QuickSwitcher {
    pub fn new(
        files: Vec<VaultPath>,
        recent: Vec<VaultPath>,
        on_choose: impl Fn(Choice) + 'static,
    ) -> Self {
        let switcher: Self = glib::Object::new();
        let imp = switcher.imp();
        imp.files.replace(files);
        imp.recent.replace(recent);
        imp.on_choose.replace(Some(Rc::new(on_choose)));
        switcher.update();
        switcher
    }

    fn selection(&self) -> gtk::SingleSelection {
        self.imp().selection.borrow().clone().unwrap()
    }

    fn query(&self) -> String {
        self.imp().search_entry.text().trim().to_owned()
    }

    fn update(&self) {
        let imp = self.imp();
        let query = self.query();
        let ranked = rank(&query, &imp.files.borrow(), &imp.recent.borrow(), LIMIT);
        let strings: Vec<&str> = ranked.iter().map(VaultPath::as_str).collect();
        imp.results.splice(0, imp.results.n_items(), &strings);
        if ranked.is_empty() {
            imp.empty_page.set_description(Some(&if query.is_empty() {
                "This vault has no notes yet".to_owned()
            } else {
                format!(
                    "Press Shift+Enter to create “{}”",
                    glib::markup_escape_text(&query)
                )
            }));
            imp.stack.set_visible_child_name("empty");
        } else {
            imp.stack.set_visible_child_name("results");
            self.selection().set_selected(0);
            imp.results_view
                .scroll_to(0, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn on_key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let selection = self.selection();
        let n = selection.n_items();
        match key {
            gdk::Key::Down | gdk::Key::Up if n > 0 => {
                let current = selection.selected();
                let next = match (key, current) {
                    (_, gtk::INVALID_LIST_POSITION) => 0,
                    (gdk::Key::Down, i) => (i + 1).min(n - 1),
                    (_, i) => i.saturating_sub(1),
                };
                selection.set_selected(next);
                self.imp()
                    .results_view
                    .scroll_to(next, gtk::ListScrollFlags::NONE, None);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                if state.contains(gdk::ModifierType::SHIFT_MASK) {
                    let name = self.query();
                    if !name.is_empty() {
                        self.finish(Choice::Create { name });
                    }
                } else if selection.selected() != gtk::INVALID_LIST_POSITION {
                    let new_tab = state.contains(gdk::ModifierType::CONTROL_MASK);
                    self.choose_at(selection.selected(), new_tab);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn choose_at(&self, position: u32, new_tab: bool) {
        let Some(path) = self.imp().results.string(position) else {
            return;
        };
        if let Ok(path) = VaultPath::new(&path) {
            self.finish(Choice::Open { path, new_tab });
        }
    }

    fn finish(&self, choice: Choice) {
        let on_choose = self.imp().on_choose.borrow().clone();
        self.close();
        if let Some(f) = on_choose {
            f(choice);
        }
    }
}

fn row_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let folder = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .css_classes(["dim-label", "caption"])
            .build();
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        row.append(&title);
        row.append(&folder);
        item.set_child(Some(&row));
    });
    factory.connect_bind(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(path) = item.item().and_downcast::<gtk::StringObject>() else {
            return;
        };
        let Ok(path) = VaultPath::new(&path.string()) else {
            return;
        };
        let row = item.child().and_downcast::<gtk::Box>().unwrap();
        let title = row.first_child().and_downcast::<gtk::Label>().unwrap();
        let folder = title.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let (name, ext) = crate::files::display_name(&path, false);
        title.set_label(&match ext {
            Some(ext) => format!("{name}.{ext}"),
            None => name,
        });
        let parent = path.parent().map(|p| p.to_string());
        folder.set_visible(parent.is_some());
        folder.set_label(parent.as_deref().unwrap_or(""));
    });
    factory
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<VaultPath> {
        list.iter().map(|p| VaultPath::new(p).unwrap()).collect()
    }

    #[test]
    fn empty_query_lists_recent_first() {
        let files = paths(&["b.md", "a.md", "c/d.md"]);
        let recent = paths(&["c/d.md", "gone.md"]);
        let ranked = rank("", &files, &recent, 10);
        assert_eq!(ranked, paths(&["c/d.md", "a.md", "b.md"]));
    }

    #[test]
    fn fuzzy_matches_names_and_paths() {
        let files = paths(&[
            "projects/igneous/Roadmap.md",
            "Home.md",
            "daily/2026-10-04.md",
            "notes/Rust ownership.md",
        ]);
        let ranked = rank("road", &files, &[], 10);
        assert_eq!(
            ranked.first().map(VaultPath::as_str),
            Some("projects/igneous/Roadmap.md")
        );
        let ranked = rank("rust own", &files, &[], 10);
        assert_eq!(
            ranked.first().map(VaultPath::as_str),
            Some("notes/Rust ownership.md")
        );
        assert!(rank("zzzz", &files, &[], 10).is_empty());
        // The .md extension isn't part of the match.
        assert!(rank(".md", &files, &[], 10).is_empty());
    }

    #[test]
    fn recent_files_win_ties() {
        let files = paths(&["a/Note.md", "b/Note.md"]);
        let ranked = rank("note", &files, &paths(&["b/Note.md"]), 10);
        assert_eq!(ranked[0].as_str(), "b/Note.md");
    }
}
