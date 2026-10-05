//! The quick switcher (Ctrl+O): fuzzy-find a note by name or alias, or
//! create one. `note#heading` lists the best note's headings.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib, pango};
use igneous_core::VaultPath;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const LIMIT: usize = 200;

/// What the user picked.
pub enum Choice {
    /// `at` is a byte offset to put the cursor at (a heading).
    Open {
        path: VaultPath,
        new_tab: bool,
        at: Option<usize>,
    },
    Create {
        name: String,
    },
}

/// A result row, stored in the list model as fields joined by `\x1f`:
/// the path, then an alias or a heading and its offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Note(VaultPath),
    Alias(VaultPath, String),
    Heading(VaultPath, String, usize),
}

impl Hit {
    fn encode(&self) -> String {
        match self {
            Hit::Note(p) => p.to_string(),
            Hit::Alias(p, alias) => format!("{p}\x1f{alias}"),
            Hit::Heading(p, text, at) => format!("{p}\x1f{text}\x1f{at}"),
        }
    }

    fn decode(s: &str) -> Option<Hit> {
        let mut parts = s.split('\x1f');
        let path = VaultPath::new(parts.next()?).ok()?;
        Some(match (parts.next(), parts.next()) {
            (None, _) => Hit::Note(path),
            (Some(alias), None) => Hit::Alias(path, alias.to_owned()),
            (Some(text), Some(at)) => Hit::Heading(path, text.to_owned(), at.parse().ok()?),
        })
    }

    fn path(&self) -> &VaultPath {
        match self {
            Hit::Note(p) | Hit::Alias(p, _) | Hit::Heading(p, _, _) => p,
        }
    }
}

/// Notes matching `query` by name or alias, best first.
pub fn rank_with_aliases(
    query: &str,
    files: &[VaultPath],
    recent: &[VaultPath],
    aliases: &HashMap<VaultPath, Vec<String>>,
    limit: usize,
) -> Vec<Hit> {
    let mut hits: Vec<Hit> = rank(query, files, recent, limit)
        .into_iter()
        .map(Hit::Note)
        .collect();
    let query = query.trim();
    if query.is_empty() {
        return hits;
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut alias_hits: Vec<(u32, Hit)> = Vec::new();
    for (path, names) in aliases {
        for alias in names {
            if let Some(score) = pattern.score(Utf32Str::new(alias, &mut buf), &mut matcher) {
                alias_hits.push((score, Hit::Alias(path.clone(), alias.clone())));
            }
        }
    }
    alias_hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    // Aliases that match better than the note's own name go first.
    for (_, hit) in alias_hits.into_iter().take(limit) {
        if !hits.iter().take(3).any(|h| h.path() == hit.path()) {
            hits.insert(hits.len().min(3), hit);
        }
    }
    hits.truncate(limit);
    hits
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
type HeadingsFn = dyn Fn(&VaultPath) -> Vec<(String, u8, usize)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/h4rl33/igneous/quick-switcher.ui")]
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
        pub aliases: RefCell<HashMap<VaultPath, Vec<String>>>,
        pub headings: RefCell<Option<Rc<HeadingsFn>>>,
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
    /// `headings` lists a note's headings (text, level, byte offset) for
    /// `note#heading` queries.
    pub fn new(
        files: Vec<VaultPath>,
        recent: Vec<VaultPath>,
        aliases: HashMap<VaultPath, Vec<String>>,
        headings: impl Fn(&VaultPath) -> Vec<(String, u8, usize)> + 'static,
        on_choose: impl Fn(Choice) + 'static,
    ) -> Self {
        let switcher: Self = glib::Object::new();
        let imp = switcher.imp();
        imp.files.replace(files);
        imp.recent.replace(recent);
        imp.aliases.replace(aliases);
        imp.headings.replace(Some(Rc::new(headings)));
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
        let ranked = match query.split_once('#') {
            // `note#heading`: the headings of the best match for `note`.
            Some((note, heading)) => {
                let best = rank(note, &imp.files.borrow(), &imp.recent.borrow(), 1);
                let headings = imp.headings.borrow().clone();
                match (best.first(), headings) {
                    (Some(path), Some(headings)) => {
                        let found = headings(path);
                        let heading = heading.trim().to_lowercase();
                        found
                            .into_iter()
                            .filter(|(text, _, _)| text.to_lowercase().contains(&heading))
                            .map(|(text, level, at)| {
                                Hit::Heading(
                                    path.clone(),
                                    format!(
                                        "{}{text}",
                                        "  ".repeat(usize::from(level.saturating_sub(1)))
                                    ),
                                    at,
                                )
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                }
            }
            None => rank_with_aliases(
                &query,
                &imp.files.borrow(),
                &imp.recent.borrow(),
                &imp.aliases.borrow(),
                LIMIT,
            ),
        };
        let encoded: Vec<String> = ranked.iter().map(Hit::encode).collect();
        let strings: Vec<&str> = encoded.iter().map(String::as_str).collect();
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
        let Some(hit) = self.imp().results.string(position) else {
            return;
        };
        let Some(hit) = Hit::decode(&hit) else { return };
        let at = match &hit {
            Hit::Heading(_, _, at) => Some(*at),
            _ => None,
        };
        self.finish(Choice::Open {
            path: hit.path().clone(),
            new_tab,
            at,
        });
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
        let Some(hit) = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .and_then(|s| Hit::decode(&s.string()))
        else {
            return;
        };
        let path = hit.path();
        let row = item.child().and_downcast::<gtk::Box>().unwrap();
        let title = row.first_child().and_downcast::<gtk::Label>().unwrap();
        let folder = title.next_sibling().and_downcast::<gtk::Label>().unwrap();
        let (name, ext) = crate::files::display_name(path, false);
        let name = match ext {
            Some(ext) => format!("{name}.{ext}"),
            None => name,
        };
        let parent = path.parent().map(|p| p.to_string());
        match &hit {
            Hit::Note(_) => {
                title.set_label(&name);
                folder.set_visible(parent.is_some());
                folder.set_label(parent.as_deref().unwrap_or(""));
            }
            Hit::Alias(_, alias) => {
                title.set_label(alias);
                folder.set_visible(true);
                folder.set_label(&format!("Alias of {name}"));
            }
            Hit::Heading(_, heading, _) => {
                title.set_label(heading);
                folder.set_visible(true);
                folder.set_label(&format!("Heading in {name}"));
            }
        }
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
    fn aliases_match() {
        let files = paths(&["Projects/Igneous.md", "Home.md"]);
        let mut aliases = HashMap::new();
        aliases.insert(
            paths(&["Home.md"])[0].clone(),
            vec!["Start page".to_owned()],
        );
        let hits = rank_with_aliases("start", &files, &[], &aliases, 10);
        assert_eq!(
            hits[0],
            Hit::Alias(paths(&["Home.md"])[0].clone(), "Start page".into())
        );
        for hit in [
            Hit::Note(paths(&["a/b.md"])[0].clone()),
            Hit::Alias(paths(&["a.md"])[0].clone(), "x y".into()),
            Hit::Heading(paths(&["a.md"])[0].clone(), "Plans".into(), 42),
        ] {
            assert_eq!(Hit::decode(&hit.encode()), Some(hit));
        }
    }

    #[test]
    fn recent_files_win_ties() {
        let files = paths(&["a/Note.md", "b/Note.md"]);
        let ranked = rank("note", &files, &paths(&["b/Note.md"]), 10);
        assert_eq!(ranked[0].as_str(), "b/Note.md");
    }
}
