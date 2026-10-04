//! A read-only tab showing text that isn't a note on disk: a diff, or a note
//! as it was in an earlier commit.

use std::cell::{OnceCell, RefCell};

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;
use igneous_core::VaultPath;
use sourceview::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contents {
    /// Uncommitted changes to a file: staged ones, or the rest.
    Diff { staged: bool },
    /// The file as of a commit.
    Version { hash: String, short: String },
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TextPage {
        pub view: OnceCell<sourceview::View>,
        pub path: RefCell<Option<VaultPath>>,
        pub contents: RefCell<Option<Contents>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TextPage {
        const NAME: &'static str = "IgneousTextPage";
        type Type = super::TextPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TextPage {}
    impl WidgetImpl for TextPage {}
    impl BinImpl for TextPage {}
}

glib::wrapper! {
    pub struct TextPage(ObjectSubclass<imp::TextPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TextPage {
    pub fn new(
        path: &VaultPath,
        contents: Contents,
        text: &str,
        scheme: Option<&sourceview::StyleScheme>,
    ) -> Self {
        let page: Self = glib::Object::new();
        let language = match contents {
            Contents::Diff { .. } => "diff",
            Contents::Version { .. } => igneous_editor::LANGUAGE_ID,
        };
        let buffer = sourceview::Buffer::new(None);
        buffer.set_language(
            sourceview::LanguageManager::default()
                .language(language)
                .as_ref(),
        );
        buffer.set_style_scheme(scheme);
        buffer.set_text(text);
        let view = sourceview::View::builder()
            .buffer(&buffer)
            .editable(false)
            .monospace(matches!(contents, Contents::Diff { .. }))
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(24)
            .right_margin(24)
            .top_margin(18)
            .bottom_margin(18)
            .build();
        view.add_css_class("igneous-note");
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        page.set_child(Some(&scrolled));
        let imp = page.imp();
        imp.view.set(view).unwrap();
        imp.path.replace(Some(path.clone()));
        imp.contents.replace(Some(contents));
        page
    }

    pub fn path(&self) -> Option<VaultPath> {
        self.imp().path.borrow().clone()
    }

    pub fn contents(&self) -> Option<Contents> {
        self.imp().contents.borrow().clone()
    }

    pub fn text(&self) -> String {
        let buffer = self.imp().view.get().unwrap().buffer();
        let (start, end) = buffer.bounds();
        buffer.text(&start, &end, true).to_string()
    }

    /// The tab title, e.g. "Home (changes)" or "Home @ 1a2b3c4".
    pub fn title(&self) -> String {
        let name = self
            .path()
            .map(|p| crate::files::display_name(&p, false).0)
            .unwrap_or_default();
        match self.contents() {
            Some(Contents::Diff { staged: false }) => format!("{name} (changes)"),
            Some(Contents::Diff { staged: true }) => format!("{name} (staged)"),
            Some(Contents::Version { short, .. }) => format!("{name} @ {short}"),
            None => name,
        }
    }

    pub fn set_style_scheme(&self, scheme: Option<&sourceview::StyleScheme>) {
        if let Some(buffer) = self
            .imp()
            .view
            .get()
            .map(|v| v.buffer())
            .and_downcast::<sourceview::Buffer>()
        {
            buffer.set_style_scheme(scheme);
        }
    }
}
