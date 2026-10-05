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

/// Widest the text column gets before margins grow, in pixels, unless the
/// vault sets its own width.
pub const READABLE_WIDTH: i32 = 720;
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
    /// Headings and callouts that can fold, and the starts of folded ones.
    pub foldables: Vec<crate::fold::Foldable>,
    pub folded: std::collections::BTreeSet<usize>,
    /// Notes linking here, shown after the last line when set.
    pub mentions: Option<Vec<crate::Mention>>,
    /// Set to keep folds through the next wholesale change (a mode switch
    /// or a restyle, rather than new text).
    pub fold_kept: bool,
}

/// A rendered formula, or why it couldn't be.
pub(crate) type Formula = Result<gdk::Texture, String>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct NoteView {
        pub(crate) last_width: Cell<i32>,
        /// A relayout is scheduled (see `schedule_relayout`).
        pub(crate) relayout_pending: Cell<bool>,
        /// Add Property takes the focus once the properties show (see
        /// `start_properties`).
        pub(crate) focus_new_property: Cell<bool>,
        /// Extra top margin holding an empty note's properties, and the
        /// margin without it.
        pub(crate) empty_note_space: Cell<i32>,
        pub(crate) base_top_margin: OnceCell<i32>,
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
        pub(crate) free_slots: RefCell<Vec<crate::slot::Slot>>,
        pub(crate) input: RefCell<crate::input::InputOptions>,
        /// The theme in use and whether the desktop is dark.
        pub(crate) theme: RefCell<Option<(Theme, bool)>>,
        pub(crate) vim: RefCell<Option<gtk::EventControllerKey>>,
        /// Hands keys to widgets inside the note (see `forward_child_keys`).
        pub(crate) child_keys: RefCell<Option<gtk::EventControllerKey>>,
        pub(crate) vim_context: RefCell<Option<sourceview::VimIMContext>>,
        pub(crate) spelling: RefCell<Option<libspelling::TextBufferAdapter>>,
        /// Whether wide windows keep the text column narrow.
        pub(crate) wide: Cell<bool>,
        /// The text column's width when it's kept narrow; 0 for the default.
        pub(crate) readable_width: Cell<i32>,
        /// The family code, math source and tables use; None for monospace.
        pub(crate) monospace: RefCell<Option<String>>,
        /// Whether headings fold (callouts with a fold sign always do).
        pub(crate) fold_headings: Cell<bool>,
        /// Laid-out widths of leading whitespace (see `live.rs`).
        pub(crate) indents: RefCell<HashMap<String, i32>>,
        /// Rendered formulas by TeX, colour and size in device pixels.
        pub(crate) formulas: RefCell<HashMap<(String, String, i32), Formula>>,
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
            // Last, so it runs before every other key handler on the view.
            view.forward_child_keys();
        }
    }

    impl WidgetImpl for NoteView {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // Margins for the new width before the text is laid out at it:
            // setting them later wraps the text at the wrong width for a
            // frame, then again at the right one, which made resizing a
            // sidebar stutter. Margins only invalidate the layout, which
            // the allocation below validates anyway.
            self.obj().update_margins_for(width);
            self.parent_size_allocate(width, height, baseline);
        }
    }

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
        // Embeds and images were built by the old host.
        self.rebuild_overlays();
        self.restyle_all();
    }

    pub fn mode(&self) -> Mode {
        self.imp().state.borrow().mode
    }

    /// Colours the view with `theme`'s light or dark variant.
    pub fn set_theme(&self, theme: &Theme, dark: bool) {
        self.imp().indents.borrow_mut().clear();
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

    /// Shows `mentions` in a Linked Mentions section after the last line
    /// (Live Preview and Reading), or hides the section with `None`.
    pub fn set_linked_mentions(&self, mentions: Option<Vec<crate::Mention>>) {
        let changed = {
            let mut st = self.imp().state.borrow_mut();
            let changed = st.mentions != mentions;
            st.mentions = mentions;
            changed
        };
        if changed {
            // The section's widget is rebuilt with the new list.
            for overlay in &self.imp().state.borrow().overlays {
                if overlay.is_mentions() {
                    overlay.mark_stale();
                }
            }
            self.sync_overlays_now();
        }
    }

    /// Whether headings get fold arrows.
    pub fn set_fold_headings(&self, fold: bool) {
        if self.imp().fold_headings.replace(fold) != fold {
            self.restyle_all();
        }
    }

    /// Whether the text column stays a readable width on wide windows.
    pub fn set_readable_line_length(&self, readable: bool) {
        self.imp().wide.set(!readable);
        self.imp().last_width.set(-1);
        self.update_margins();
    }

    /// The text column's width, in pixels, with readable line length on.
    pub fn set_readable_width(&self, width: i32) {
        if self.imp().readable_width.replace(width) != width {
            self.imp().last_width.set(-1);
            self.update_margins();
        }
    }

    /// The font family for code, math source and tables; None uses the
    /// desktop's monospace font. Nested embeds follow.
    pub fn set_monospace_family(&self, family: Option<&str>) {
        let family = family.map(str::to_owned);
        if *self.imp().monospace.borrow() == family {
            return;
        }
        self.tags()
            .set_monospace_family(family.as_deref().unwrap_or("monospace"));
        self.imp().monospace.replace(family);
        self.refresh_widgets();
    }

    /// Rebuilds the widgets over the text, e.g. after the font changed: a
    /// formula is rendered at the text's size.
    pub fn refresh_widgets(&self) {
        self.rebuild_overlays();
        self.restyle_all();
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
        // Vim's handler is newer; widgets in the note still come first.
        self.forward_child_keys();
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
        self.refresh_widgets();
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
        self.update_margins_for(self.width());
    }

    /// The margins for a view `width` pixels wide.
    fn update_margins_for(&self, width: i32) {
        if width == self.imp().last_width.get() {
            return;
        }
        self.imp().last_width.set(width);
        let margin = if self.imp().wide.get() {
            MIN_MARGIN
        } else {
            let readable = match self.imp().readable_width.get() {
                0 => READABLE_WIDTH,
                width => width,
            };
            ((width - readable) / 2).max(MIN_MARGIN)
        };
        if self.left_margin() != margin {
            self.set_left_margin(margin);
            self.set_right_margin(margin);
        }
        self.tags().set_left_margin(margin);
    }
}
