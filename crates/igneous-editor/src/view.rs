//! The note view: a GtkSourceView with three modes.
//!
//! - **Live** (Live Preview): formatted text, with Markdown syntax hidden
//!   except around the cursor, and widgets for tasks, images, embeds, tables
//!   and properties.
//! - **Source**: plain Markdown with syntax highlighting.
//! - **Reading**: Live Preview, read-only, with nothing revealed.
//!
//! The buffer always holds the exact Markdown; nothing is ever inserted for
//! display. See `live.rs` for how Live Preview works, and
//! docs/decisions/0001-editor.md for why.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*, subclass::prelude::*};
use igneous_markdown::Document;
use igneous_markdown::present::StyledSpan;
use sourceview::prelude::*;
use sourceview::subclass::prelude::*;

use crate::host::{Host, NoHost};
use crate::live::Overlay;
use crate::rangeset::RangeSet;
use crate::tags::{Palette, Tags};
use crate::theme::Theme;

/// Widest the text column gets before margins grow, in pixels.
pub(crate) const READABLE_WIDTH: i32 = 720;
pub(crate) const MIN_MARGIN: i32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Live,
    Source,
    Reading,
}

/// What Live Preview knows about the text, kept in step with the buffer.
#[derive(Default)]
pub(crate) struct State {
    pub text: String,
    /// Byte offset of each line's start.
    pub lines: Vec<usize>,
    pub doc: Document,
    pub spans: Vec<StyledSpan>,
    /// The ranges each tag currently covers.
    pub applied: HashMap<String, RangeSet>,
    /// Edits since the last restyle: (position, bytes deleted, bytes inserted).
    pub pending: Vec<(usize, usize, usize)>,
    pub mode: Mode,
    pub selection: std::ops::Range<usize>,
    pub overlays: Vec<Overlay>,
    /// Frontmatter source the properties widget was built from.
    pub properties_source: String,
    /// Links whose targets don't exist.
    pub unresolved: Vec<std::ops::Range<usize>>,
    /// Problems underlined in the text (see `diagnostics.rs`).
    pub diagnostics: Vec<crate::Diagnostic>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct NoteView {
        pub(crate) last_width: Cell<i32>,
        pub(crate) tags: OnceCell<Tags>,
        pub(crate) state: RefCell<State>,
        pub(crate) host: RefCell<Option<Rc<dyn Host>>>,
        pub(crate) palette: RefCell<Option<Palette>>,
        /// Set while Igneous itself changes a checkbox, so it isn't taken as
        /// a click.
        pub(crate) syncing: Cell<bool>,
        pub(crate) textures: RefCell<HashMap<String, Option<gdk::Texture>>>,
        pub(crate) checkbox_height: Cell<i32>,
        /// How deeply this view is nested in note embeds (0 for a tab).
        pub(crate) depth: Cell<u8>,
        /// Overlay slots not showing anything (see `live.rs`).
        pub(crate) free_slots: RefCell<Vec<adw::Bin>>,
        pub(crate) input: RefCell<crate::input::InputOptions>,
        /// The theme in use and whether the desktop is dark.
        pub(crate) theme: RefCell<Option<(Theme, bool)>>,
        pub(crate) vim: RefCell<Option<gtk::EventControllerKey>>,
        pub(crate) vim_context: RefCell<Option<sourceview::VimIMContext>>,
        pub(crate) spelling: RefCell<Option<libspelling::TextBufferAdapter>>,
        /// Whether wide windows keep the text column narrow.
        pub(crate) wide: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NoteView {
        const NAME: &'static str = "IgneousNoteView";
        type Type = super::NoteView;
        type ParentType = sourceview::View;
    }

    impl ObjectImpl for NoteView {
        fn constructed(&self) {
            self.parent_constructed();
            crate::init();
            let view = self.obj();
            let tags = Tags::new();
            let buffer = sourceview::Buffer::new(Some(&tags.table));
            buffer.set_language(
                sourceview::LanguageManager::default()
                    .language(crate::LANGUAGE_ID)
                    .as_ref(),
            );
            buffer.set_highlight_matching_brackets(false);
            // Always on: spellcheck needs the syntax engine's context classes
            // even in Live Preview, whose scheme has no syntax colours.
            buffer.set_highlight_syntax(true);
            view.set_buffer(Some(&buffer));
            self.tags.set(tags).ok().unwrap();
            self.state.borrow_mut().lines = vec![0];

            view.set_wrap_mode(gtk::WrapMode::WordChar);
            view.set_top_margin(24);
            view.set_bottom_margin(96);
            view.set_left_margin(MIN_MARGIN);
            view.set_right_margin(MIN_MARGIN);
            view.set_pixels_below_lines(2);
            view.add_css_class("igneous-note");
            view.connect_live_preview(&buffer);
            view.connect_input();
            let completion = view.completion();
            completion.add_provider(&crate::completion::LinkCompletion::new(&view));
            completion.set_select_on_show(true);
            view.hover()
                .add_provider(&crate::hover::LinkPreview::new(&view));
            view.hover()
                .add_provider(&crate::diagnostics::ProblemHover::new(&view));
        }
    }

    impl WidgetImpl for NoteView {}

    impl TextViewImpl for NoteView {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            if layer == gtk::TextViewLayer::BelowText {
                self.obj().draw_decor(&snapshot);
            }
        }
    }

    impl ViewImpl for NoteView {}
}

glib::wrapper! {
    pub struct NoteView(ObjectSubclass<imp::NoteView>)
        @extends sourceview::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl Default for NoteView {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteView {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// The view's own buffer (it creates one with Live Preview's tags).
    pub fn source_buffer(&self) -> sourceview::Buffer {
        self.buffer()
            .downcast()
            .expect("a note view's buffer is a GtkSourceView buffer")
    }

    pub(crate) fn tags(&self) -> &Tags {
        self.imp().tags.get().expect("tags exist once constructed")
    }

    pub(crate) fn host(&self) -> Rc<dyn Host> {
        self.imp()
            .host
            .borrow()
            .clone()
            .unwrap_or_else(|| Rc::new(NoHost))
    }

    /// Connects the view to the app: link targets, images and embeds.
    pub fn set_host(&self, host: Rc<dyn Host>) {
        self.imp().host.replace(Some(host));
        self.imp().textures.borrow_mut().clear();
        self.restyle_all();
    }

    pub fn mode(&self) -> Mode {
        self.imp().state.borrow().mode
    }

    /// Colours the view with `theme`'s light or dark variant.
    pub fn set_theme(&self, theme: &Theme, dark: bool) {
        self.imp().theme.replace(Some((theme.clone(), dark)));
        self.apply_scheme();
        let palette = Palette::new(theme.variant(dark), crate::system_accent());
        self.tags().set_palette(&palette);
        self.imp().palette.replace(Some(palette));
        // Widgets built with the old colours (tables, embeds) are rebuilt.
        self.rebuild_overlays();
        self.queue_draw();
    }

    /// Source mode shows the theme's syntax colours; Live Preview and Reading
    /// use the same theme without them (their tags do the styling).
    pub(crate) fn apply_scheme(&self) {
        let Some((theme, dark)) = self.imp().theme.borrow().clone() else {
            return;
        };
        let scheme = if self.mode() == Mode::Source {
            crate::style_scheme(&theme, dark)
        } else {
            crate::scheme::live_style_scheme(&theme, dark)
        };
        self.source_buffer().set_style_scheme(scheme.as_ref());
    }

    /// Whether the text column stays a readable width on wide windows.
    pub fn set_readable_line_length(&self, readable: bool) {
        self.imp().wide.set(!readable);
        self.imp().last_width.set(-1);
        self.update_margins();
    }

    /// Vim keybindings (GtkSourceView's emulation).
    pub fn set_vim_mode(&self, enabled: bool) {
        let imp = self.imp();
        if enabled == imp.vim.borrow().is_some() {
            return;
        }
        if let Some(keys) = imp.vim.take() {
            self.remove_controller(&keys);
            imp.vim_context.take();
            return;
        }
        let context = sourceview::VimIMContext::new();
        let keys = gtk::EventControllerKey::new();
        keys.set_im_context(Some(&context));
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        self.add_controller(keys.clone());
        context.set_client_widget(Some(self));
        imp.vim.replace(Some(keys));
        imp.vim_context.replace(Some(context));
    }

    /// The Vim emulation, while it's on (for its command bar and `:w`).
    pub fn vim_context(&self) -> Option<sourceview::VimIMContext> {
        self.imp().vim_context.borrow().clone()
    }

    /// Underlines misspelt words (libspelling, in the desktop's language);
    /// corrections are in the context menu.
    pub fn set_spellcheck(&self, enabled: bool) {
        let imp = self.imp();
        if imp.spelling.borrow().is_none() {
            if !enabled {
                return;
            }
            let adapter = libspelling::TextBufferAdapter::new(
                &self.source_buffer(),
                &libspelling::Checker::default(),
            );
            self.set_extra_menu(Some(&adapter.menu_model()));
            self.insert_action_group("spelling", Some(&adapter));
            imp.spelling.replace(Some(adapter));
        }
        if let Some(adapter) = imp.spelling.borrow().as_ref() {
            adapter.set_enabled(enabled);
        }
    }

    /// Checks link targets again, e.g. after files were added or removed,
    /// and reloads images and embeds.
    pub fn refresh_links(&self) {
        self.imp().textures.borrow_mut().clear();
        self.rebuild_overlays();
        self.restyle_all();
    }

    /// The text as Live Preview last saw it.
    pub fn model_text(&self) -> String {
        self.imp().state.borrow().text.clone()
    }

    /// Checks that the buffer still holds exactly what Live Preview thinks it
    /// does. For tests.
    pub fn check_invariants(&self) -> Result<(), String> {
        let buffer = self.buffer();
        let (a, b) = buffer.bounds();
        let text = buffer.text(&a, &b, true);
        let state = self.imp().state.borrow();
        if text.as_str() != state.text {
            return Err(format!(
                "buffer ({} bytes) differs from model ({} bytes)",
                text.len(),
                state.text.len()
            ));
        }
        if !state.pending.is_empty() {
            return Err("unprocessed edits".into());
        }
        Ok(())
    }

    /// Byte ranges hidden right now. For tests.
    pub fn concealed(&self) -> Vec<std::ops::Range<usize>> {
        self.imp()
            .state
            .borrow()
            .applied
            .get("Conceal")
            .map(|s| s.ranges().to_vec())
            .unwrap_or_default()
    }

    pub(crate) fn set_depth(&self, depth: u8) {
        self.imp().depth.set(depth);
    }

    pub(crate) fn depth(&self) -> u8 {
        self.imp().depth.get()
    }

    /// Keeps the text column readable: grows the margins on wide windows.
    pub(crate) fn update_margins(&self) {
        let width = self.width();
        if width == self.imp().last_width.get() {
            return;
        }
        self.imp().last_width.set(width);
        let margin = if self.imp().wide.get() {
            MIN_MARGIN
        } else {
            ((width - READABLE_WIDTH) / 2).max(MIN_MARGIN)
        };
        if self.left_margin() != margin {
            self.set_left_margin(margin);
            self.set_right_margin(margin);
        }
        self.tags().set_left_margin(margin);
    }
}
