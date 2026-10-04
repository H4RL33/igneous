//! Live Preview on a GtkSourceView buffer.
//!
//! After every change the note is re-parsed, the presentation spans are
//! recomputed, and only the tag differences are applied. Cursor and selection
//! changes recompute concealment only. The buffer always holds the exact
//! Markdown: nothing is ever inserted for presentation.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use igneous_markdown::present::{self, Replacement, Style, StyledSpan};
use igneous_markdown::{Document, frontmatter::Value, parse};
use sourceview::prelude::*;

use crate::rangeset::RangeSet;
use crate::tags::{Concealment, Palette, TagKind, Tags};
use crate::view::LpView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Live,
    Source,
    Reading,
}

#[derive(Default)]
pub struct Stats {
    /// Time spent re-parsing and re-tagging after each change.
    pub restyle: Vec<Duration>,
    /// Time spent updating concealment after each cursor move.
    pub reveal: Vec<Duration>,
    /// Overlay moves and attach/detach operations.
    pub overlay_moves: Cell<usize>,
    pub overlay_attach: Cell<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum OverlayKind {
    Checkbox,
    Image(String),
    Properties,
}

/// A widget shown over the text. Widgets exist only while near the screen:
/// GTK re-allocates every text view child on each layout, so hundreds of
/// off-screen checkboxes would make every frame slow.
struct Overlay {
    kind: OverlayKind,
    mark: gtk::TextMark,
    checked: Cell<bool>,
    widget: RefCell<Option<gtk::Widget>>,
    attached: Cell<bool>,
    height: i32,
    last_pos: Cell<(i32, i32)>,
}

struct State {
    text: String,
    lines: Vec<usize>,
    doc: Document,
    spans: Vec<StyledSpan>,
    applied: HashMap<TagKind, RangeSet>,
    spacing: HashMap<String, RangeSet>,
    pending: Vec<(usize, usize, usize)>,
    mode: Mode,
    selection: Range<usize>,
    overlays: Vec<Overlay>,
    /// Frontmatter source the properties widget was built from.
    properties_source: String,
    stats: Stats,
}

pub struct Controller {
    pub view: LpView,
    pub buffer: sourceview::Buffer,
    tags: Tags,
    palette: Palette,
    state: RefCell<State>,
    note_dir: PathBuf,
    syncing: Cell<bool>,
    this: RefCell<Weak<Controller>>,
    /// Plain GtkSourceView: no Live Preview work at all (for comparison).
    baseline: Cell<bool>,
    textures: RefCell<HashMap<String, Option<gdk::Texture>>>,
    checkbox_height: Cell<i32>,
}

/// Diagnostic switches for bisecting frame cost: IGNEOUS_SPIKE_OFF=overlays,decor,conceal
fn off(part: &str) -> bool {
    std::env::var("IGNEOUS_SPIKE_OFF").is_ok_and(|v| v.split(',').any(|p| p == part))
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

fn byte_of(lines: &[usize], iter: &gtk::TextIter) -> usize {
    lines.get(iter.line() as usize).copied().unwrap_or(0) + iter.line_index() as usize
}

impl Controller {
    pub fn new(text: &str, note_dir: PathBuf, concealment: Concealment) -> Rc<Self> {
        let style = adw::StyleManager::default();
        let dark = style.is_dark();
        let palette = Palette {
            fg: if dark {
                gdk::RGBA::new(1.0, 1.0, 1.0, 0.92)
            } else {
                gdk::RGBA::new(0.0, 0.0, 0.0, 0.8)
            },
            accent: style.accent_color_rgba(),
        };
        let tags = Tags::new(&palette, concealment);
        let buffer = sourceview::Buffer::new(Some(&tags.table));
        buffer.set_highlight_syntax(false);
        buffer.set_highlight_matching_brackets(false);
        let schemes = sourceview::StyleSchemeManager::default();
        buffer.set_style_scheme(
            schemes
                .scheme(if dark { "Adwaita-dark" } else { "Adwaita" })
                .as_ref(),
        );
        buffer.set_text(text);

        let view = LpView::new(&buffer);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_top_margin(24);
        view.set_bottom_margin(64);
        view.set_pixels_below_lines(3);

        let this = Rc::new(Self {
            view,
            buffer,
            tags,
            palette,
            state: RefCell::new(State {
                text: String::new(),
                lines: vec![0],
                doc: Document::default(),
                spans: Vec::new(),
                applied: HashMap::new(),
                spacing: HashMap::new(),
                pending: Vec::new(),
                mode: Mode::Live,
                selection: 0..0,
                overlays: Vec::new(),
                properties_source: String::new(),
                stats: Stats::default(),
            }),
            note_dir,
            syncing: Cell::new(false),
            this: RefCell::new(Weak::new()),
            baseline: Cell::new(false),
            textures: RefCell::default(),
            checkbox_height: Cell::new(0),
        });
        this.this.replace(Rc::downgrade(&this));
        this.connect();
        this.restyle();
        this
    }

    fn connect(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.buffer.connect_insert_text(move |_, iter, text| {
            if let Some(c) = weak.upgrade() {
                let mut st = c.state.borrow_mut();
                let pos = byte_of(&st.lines, iter);
                st.pending.push((pos, 0, text.len()));
            }
        });
        let weak = Rc::downgrade(self);
        self.buffer.connect_delete_range(move |_, start, end| {
            if let Some(c) = weak.upgrade() {
                let mut st = c.state.borrow_mut();
                let (a, b) = (byte_of(&st.lines, start), byte_of(&st.lines, end));
                st.pending.push((a.min(b), a.abs_diff(b), 0));
            }
        });
        let weak = Rc::downgrade(self);
        self.buffer.connect_changed(move |_| {
            if let Some(c) = weak.upgrade() {
                c.restyle();
            }
        });
        let weak = Rc::downgrade(self);
        self.buffer.connect_mark_set(move |buffer, _, mark| {
            if let Some(c) = weak.upgrade()
                && (mark == &buffer.get_insert() || mark == &buffer.selection_bound())
            {
                c.update_reveal();
            }
        });
        let weak = Rc::downgrade(self);
        self.view.set_decor(Box::new(move |view, snapshot| {
            if let Some(c) = weak.upgrade() {
                c.draw_decor(view, snapshot);
            }
        }));
        let weak = Rc::downgrade(self);
        self.view
            .add_tick_callback(move |_, _| match weak.upgrade() {
                Some(c) => {
                    c.tick();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
    }

    // --- positions -----------------------------------------------------------

    fn iter_at(&self, lines: &[usize], byte: usize) -> gtk::TextIter {
        let line = lines.partition_point(|&s| s <= byte).saturating_sub(1);
        let index = byte - lines[line];
        self.buffer
            .iter_at_line_index(line as i32, index as i32)
            .unwrap_or_else(|| self.buffer.end_iter())
    }

    fn selection_bytes(&self, lines: &[usize]) -> Range<usize> {
        let a = byte_of(lines, &self.buffer.iter_at_mark(&self.buffer.get_insert()));
        let b = byte_of(
            lines,
            &self.buffer.iter_at_mark(&self.buffer.selection_bound()),
        );
        a.min(b)..a.max(b)
    }

    /// The first task checkbox currently on screen, if any.
    pub fn first_checkbox(&self) -> Option<gtk::CheckButton> {
        let st = self.state.borrow();
        st.overlays
            .iter()
            .filter(|o| o.kind == OverlayKind::Checkbox && o.attached.get())
            .find_map(|o| {
                o.widget
                    .borrow()
                    .as_ref()?
                    .clone()
                    .downcast::<gtk::CheckButton>()
                    .ok()
            })
    }

    /// Whether the current note hit the parser fallback.
    pub fn degraded(&self) -> bool {
        self.state.borrow().doc.degraded
    }

    pub fn text(&self) -> String {
        self.state.borrow().text.clone()
    }

    pub fn with_stats<T>(&self, f: impl FnOnce(&Stats) -> T) -> T {
        f(&self.state.borrow().stats)
    }

    pub fn clear_stats(&self) {
        self.state.borrow_mut().stats = Stats::default();
    }

    /// Byte ranges currently hidden by the Conceal tag.
    pub fn concealed(&self) -> Vec<Range<usize>> {
        let st = self.state.borrow();
        st.applied
            .get(&TagKind::Conceal)
            .map(|s| s.ranges().to_vec())
            .unwrap_or_default()
    }

    // --- restyling -----------------------------------------------------------

    /// Turns all Live Preview work off, leaving a plain GtkSourceView.
    pub fn set_baseline(&self) {
        self.set_mode(Mode::Source);
        let (from, to) = self.buffer.bounds();
        for kind in TagKind::ALL {
            self.buffer.remove_tag(self.tags.get(kind), &from, &to);
        }
        let mut st = self.state.borrow_mut();
        st.applied.clear();
        st.spans.clear();
        drop(st);
        self.baseline.set(true);
    }

    pub fn restyle(&self) {
        let start = Instant::now();
        let (a, b) = self.buffer.bounds();
        let text = self.buffer.text(&a, &b, true).to_string();
        if self.baseline.get() {
            let mut st = self.state.borrow_mut();
            st.pending.clear();
            st.lines = line_starts(&text);
            st.text = text;
            st.stats.restyle.push(start.elapsed());
            return;
        }
        {
            let mut guard = self.state.borrow_mut();
            let st = &mut *guard;
            let edits = std::mem::take(&mut st.pending);
            st.text = text;
            st.lines = line_starts(&st.text);
            if let [(pos, deleted, inserted)] = edits[..] {
                for set in st.applied.values_mut().chain(st.spacing.values_mut()) {
                    set.apply_edit(pos, deleted, inserted);
                }
                if inserted > 0 {
                    // GTK gives inserted text the tags around it; strip ours so
                    // the buffer matches the model.
                    let (from, to) = (
                        self.iter_at(&st.lines, pos),
                        self.iter_at(&st.lines, pos + inserted),
                    );
                    for kind in TagKind::ALL {
                        self.buffer.remove_tag(self.tags.get(kind), &from, &to);
                    }
                    for name in st.spacing.keys() {
                        self.buffer.remove_tag(&self.tags.spacing(name), &from, &to);
                    }
                }
            } else {
                let (from, to) = self.buffer.bounds();
                for kind in TagKind::ALL {
                    self.buffer.remove_tag(self.tags.get(kind), &from, &to);
                }
                for name in st.spacing.keys() {
                    self.buffer.remove_tag(&self.tags.spacing(name), &from, &to);
                }
                st.applied.clear();
                st.spacing.clear();
            }
            st.doc = parse(&st.text);
            st.spans = present::spans(&st.doc, &st.text);
            st.selection = self.selection_bytes(&st.lines);
            let mut desired = self.style_coverage(st);
            self.add_concealment(st, &mut desired);
            self.apply(st, desired, &TagKind::ALL);
        }
        self.sync_overlays();
        self.state.borrow_mut().stats.restyle.push(start.elapsed());
        self.view.queue_draw();
    }

    fn style_coverage(&self, st: &State) -> HashMap<TagKind, Vec<Range<usize>>> {
        let mut map: HashMap<TagKind, Vec<Range<usize>>> = HashMap::new();
        for span in &st.spans {
            if let Some(kind) = TagKind::for_style(&span.style) {
                map.entry(kind).or_default().push(span.range.clone());
            }
            map.entry(TagKind::Syntax)
                .or_default()
                .extend(span.markers.iter().cloned());
        }
        map
    }

    fn add_concealment(&self, st: &State, map: &mut HashMap<TagKind, Vec<Range<usize>>>) {
        map.entry(TagKind::Conceal).or_default();
        map.entry(TagKind::Ghost).or_default();
        if st.mode == Mode::Source || off("conceal") {
            return;
        }
        for span in &st.spans {
            let revealed = st.mode == Mode::Live && span.reveal.is_revealed_by(&st.selection);
            if !revealed {
                let ghost = matches!(
                    span.replace,
                    Some(Replacement::Checkbox { .. } | Replacement::Bullet | Replacement::Rule)
                );
                let kind = if ghost {
                    TagKind::Ghost
                } else {
                    TagKind::Conceal
                };
                map.entry(kind)
                    .or_default()
                    .extend(span.markers.iter().cloned());
            }
            if st.mode == Mode::Reading && matches!(span.style, Style::Comment | Style::BlockId) {
                map.entry(TagKind::Conceal)
                    .or_default()
                    .push(span.range.clone());
            }
        }
    }

    fn apply(
        &self,
        st: &mut State,
        mut desired: HashMap<TagKind, Vec<Range<usize>>>,
        kinds: &[TagKind],
    ) {
        let State { lines, applied, .. } = st;
        for kind in kinds {
            let new = RangeSet::from_ranges(desired.remove(kind).unwrap_or_default());
            let old = applied.entry(*kind).or_default();
            let tag = self.tags.get(*kind);
            for r in old.difference(&new) {
                self.buffer.remove_tag(
                    tag,
                    &self.iter_at(lines, r.start),
                    &self.iter_at(lines, r.end),
                );
            }
            for r in new.difference(old) {
                self.buffer.apply_tag(
                    tag,
                    &self.iter_at(lines, r.start),
                    &self.iter_at(lines, r.end),
                );
            }
            *old = new;
        }
    }

    fn update_reveal(&self) {
        if self.baseline.get() {
            return;
        }
        let start = Instant::now();
        let guard_target = {
            let Ok(mut guard) = self.state.try_borrow_mut() else {
                return;
            };
            let st = &mut *guard;
            let selection = self.selection_bytes(&st.lines);
            if selection == st.selection {
                return;
            }
            st.selection = selection;
            let mut desired = HashMap::new();
            self.add_concealment(st, &mut desired);
            self.apply(st, desired, &[TagKind::Conceal, TagKind::Ghost]);
            st.stats.reveal.push(start.elapsed());
            self.cursor_guard(st)
        };
        self.view.queue_draw();
        if let Some(target) = guard_target {
            let st = self.state.borrow();
            let iter = self.iter_at(&st.lines, target);
            drop(st);
            self.buffer.place_cursor(&iter);
        }
    }

    /// Keeps the cursor out of text that is hidden and never revealed (the
    /// frontmatter under the properties header).
    fn cursor_guard(&self, st: &State) -> Option<usize> {
        if st.mode != Mode::Live || !st.selection.is_empty() {
            return None;
        }
        let pos = st.selection.start;
        st.spans
            .iter()
            .filter(|s| s.reveal == present::Reveal::Never)
            .flat_map(|s| s.markers.iter())
            .find(|m| m.start <= pos && pos < m.end)
            .map(|m| m.end)
    }

    pub fn set_mode(&self, mode: Mode) {
        {
            let mut guard = self.state.borrow_mut();
            let st = &mut *guard;
            st.mode = mode;
            let mut desired = HashMap::new();
            self.add_concealment(st, &mut desired);
            self.apply(st, desired, &[TagKind::Conceal, TagKind::Ghost]);
        }
        self.view.set_editable(mode != Mode::Reading);
        self.view.set_cursor_visible(mode != Mode::Reading);
        self.sync_overlays();
        self.view.queue_draw();
    }

    /// Checks that the buffer still holds exactly the text the model has.
    pub fn check_invariants(&self) -> Result<(), String> {
        let (a, b) = self.buffer.bounds();
        let buffer_text = self.buffer.text(&a, &b, true);
        let st = self.state.borrow();
        if buffer_text.as_str() != st.text {
            return Err(format!(
                "buffer ({} bytes) differs from model ({} bytes)",
                buffer_text.len(),
                st.text.len()
            ));
        }
        if !st.pending.is_empty() {
            return Err("unprocessed edits".into());
        }
        Ok(())
    }

    // --- overlays ------------------------------------------------------------

    fn sync_overlays(&self) {
        let (desired, fm_end) = {
            let st = self.state.borrow();
            let mut desired: Vec<(OverlayKind, usize, bool)> = Vec::new();
            if st.mode != Mode::Source && !off("overlays") {
                for span in &st.spans {
                    match &span.replace {
                        Some(Replacement::Checkbox { checked, .. }) => {
                            desired.push((OverlayKind::Checkbox, span.range.start, *checked))
                        }
                        Some(Replacement::Image { target, .. }) => desired.push((
                            OverlayKind::Image(target.clone()),
                            span.range.start,
                            false,
                        )),
                        Some(Replacement::Properties) => {
                            desired.push((OverlayKind::Properties, 0, false))
                        }
                        _ => {}
                    }
                }
            }
            (desired, st.doc.frontmatter.as_ref().map(|f| f.range.end))
        };

        let mut guard = self.state.borrow_mut();
        let st = &mut *guard;
        let fm_source = st
            .doc
            .frontmatter
            .as_ref()
            .map(|f| st.text[f.range.clone()].to_owned())
            .unwrap_or_default();
        let mut keep = vec![false; st.overlays.len()];
        let mut create = Vec::new();
        for (kind, anchor, checked) in desired {
            let found = st.overlays.iter().enumerate().position(|(i, o)| {
                !keep[i]
                    && o.kind == kind
                    && byte_of(&st.lines, &self.buffer.iter_at_mark(&o.mark)) == anchor
            });
            match found {
                Some(_) if kind == OverlayKind::Properties && fm_source != st.properties_source => {
                    // Properties changed: rebuild the widget.
                    create.push((kind, anchor, checked));
                }
                Some(i) => {
                    keep[i] = true;
                    let overlay = &st.overlays[i];
                    overlay.checked.set(checked);
                    if let Some(check) = overlay
                        .widget
                        .borrow()
                        .as_ref()
                        .and_then(|w| w.downcast_ref::<gtk::CheckButton>())
                    {
                        self.syncing.set(true);
                        check.set_active(checked);
                        self.syncing.set(false);
                    }
                }
                None => create.push((kind, anchor, checked)),
            }
        }
        let mut kept = Vec::new();
        for (overlay, keep) in st.overlays.drain(..).zip(keep) {
            if keep {
                kept.push(overlay);
            } else {
                if overlay.attached.get()
                    && let Some(widget) = overlay.widget.borrow().as_ref()
                {
                    self.view.remove(widget);
                }
                self.buffer.delete_mark(&overlay.mark);
            }
        }
        st.overlays = kept;
        for (kind, anchor, checked) in create {
            let (widget, height) = match &kind {
                OverlayKind::Checkbox => (None, self.checkbox_height()),
                OverlayKind::Image(target) => (None, self.image_size(target).1),
                OverlayKind::Properties => {
                    let widget = self.properties_widget(&st.doc);
                    let height = widget.measure(gtk::Orientation::Vertical, 560).1;
                    st.properties_source = fm_source.clone();
                    (Some(widget), height)
                }
            };
            let mark = self
                .buffer
                .create_mark(None, &self.iter_at(&st.lines, anchor), true);
            st.overlays.push(Overlay {
                kind,
                mark,
                checked: Cell::new(checked),
                widget: RefCell::new(widget),
                attached: Cell::new(false),
                height,
                last_pos: Cell::new((-1, -1)),
            });
        }

        // Reserve vertical space for block overlays.
        let mut spacing: HashMap<String, Vec<Range<usize>>> = HashMap::new();
        for overlay in &st.overlays {
            let anchor = byte_of(&st.lines, &self.buffer.iter_at_mark(&overlay.mark));
            match overlay.kind {
                OverlayKind::Image(_) => {
                    let start = igneous_markdown::text::line_start(&st.text, anchor);
                    let end =
                        (igneous_markdown::text::line_end(&st.text, anchor) + 1).min(st.text.len());
                    spacing
                        .entry(format!("below-{}", overlay.height + 12))
                        .or_default()
                        .push(start..end.max(start + 1).min(st.text.len()));
                }
                OverlayKind::Properties => {
                    let body = fm_end.unwrap_or(0);
                    if body < st.text.len() {
                        let end = (igneous_markdown::text::line_end(&st.text, body) + 1)
                            .min(st.text.len());
                        spacing
                            .entry(format!("above-{}", overlay.height + 20))
                            .or_default()
                            .push(body..end.max(body + 1));
                    }
                }
                OverlayKind::Checkbox => {}
            }
        }
        let names: Vec<String> = st.spacing.keys().chain(spacing.keys()).cloned().collect();
        for name in names {
            let new = RangeSet::from_ranges(spacing.remove(&name).unwrap_or_default());
            let old = st.spacing.entry(name.clone()).or_default();
            if *old == new {
                continue;
            }
            let tag = self.tags.spacing(&name);
            for r in old.difference(&new) {
                self.buffer.remove_tag(
                    &tag,
                    &self.iter_at(&st.lines, r.start),
                    &self.iter_at(&st.lines, r.end),
                );
            }
            for r in new.difference(old) {
                self.buffer.apply_tag(
                    &tag,
                    &self.iter_at(&st.lines, r.start),
                    &self.iter_at(&st.lines, r.end),
                );
            }
            *old = new;
        }
    }

    fn checkbox_height(&self) -> i32 {
        if self.checkbox_height.get() == 0 {
            let probe = gtk::CheckButton::new();
            self.checkbox_height
                .set(probe.measure(gtk::Orientation::Vertical, -1).1);
        }
        self.checkbox_height.get()
    }

    fn texture(&self, target: &str) -> Option<gdk::Texture> {
        self.textures
            .borrow_mut()
            .entry(target.to_owned())
            .or_insert_with(|| {
                find_file(&self.note_dir, target)
                    .and_then(|path| gdk::Texture::from_filename(path).ok())
            })
            .clone()
    }

    /// Display size of an embedded image.
    fn image_size(&self, target: &str) -> (i32, i32) {
        match self.texture(target) {
            Some(t) => {
                let (w, h) = (t.width() as f64, t.height() as f64);
                let width = w.min(480.0);
                (width as i32, (h * width / w).round() as i32)
            }
            None => (240, 24),
        }
    }

    fn make_widget(&self, overlay: &Overlay, doc: &Document) -> gtk::Widget {
        match &overlay.kind {
            OverlayKind::Checkbox => self.checkbox_widget(overlay.checked.get()),
            OverlayKind::Image(target) => self.image_widget(target),
            OverlayKind::Properties => self.properties_widget(doc),
        }
    }

    fn checkbox_widget(&self, checked: bool) -> gtk::Widget {
        let check = gtk::CheckButton::new();
        check.set_active(checked);
        check.set_focus_on_click(false);
        check.set_can_focus(false);
        let weak = self.this.borrow().clone();
        check.connect_toggled(move |check| {
            if let Some(c) = weak.upgrade()
                && !c.syncing.get()
            {
                c.toggle_task_at(check.upcast_ref());
            }
        });
        check.upcast()
    }

    fn toggle_task_at(&self, widget: &gtk::Widget) {
        let edit = {
            let st = self.state.borrow();
            let Some(overlay) = st
                .overlays
                .iter()
                .find(|o| o.widget.borrow().as_ref() == Some(widget))
            else {
                return;
            };
            let anchor = byte_of(&st.lines, &self.buffer.iter_at_mark(&overlay.mark));
            st.spans.iter().find_map(|s| match s.replace {
                Some(Replacement::Checkbox {
                    checked,
                    status_offset,
                }) if s.range.start == anchor => {
                    let len = st.text[status_offset..]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
                    Some((
                        self.iter_at(&st.lines, status_offset).offset(),
                        self.iter_at(&st.lines, status_offset + len).offset(),
                        if checked { " " } else { "x" },
                    ))
                }
                _ => None,
            })
        };
        if let Some((from, to, replacement)) = edit {
            self.buffer.begin_user_action();
            let mut a = self.buffer.iter_at_offset(from);
            let mut b = self.buffer.iter_at_offset(to);
            self.buffer.delete(&mut a, &mut b);
            let mut at = self.buffer.iter_at_offset(from);
            self.buffer.insert(&mut at, replacement);
            self.buffer.end_user_action();
        }
    }

    fn image_widget(&self, target: &str) -> gtk::Widget {
        match self.texture(target) {
            Some(texture) => {
                let (width, height) = self.image_size(target);
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_size_request(width, height);
                picture.set_can_shrink(true);
                picture.set_content_fit(gtk::ContentFit::Contain);
                picture.add_css_class("card");
                picture.upcast()
            }
            None => {
                let label = gtk::Label::new(Some(&format!("Missing image: {target}")));
                label.add_css_class("dim-label");
                label.upcast()
            }
        }
    }

    fn properties_widget(&self, doc: &Document) -> gtk::Widget {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_size_request(560, -1);
        if let Some(fm) = &doc.frontmatter {
            for entry in &fm.entries {
                let value = match &entry.value {
                    Value::Null => String::new(),
                    Value::List(_) => entry.value.string_list().join(", "),
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Int(i) => i.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Map(_) => "{…}".into(),
                };
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&entry.key))
                    .subtitle(glib::markup_escape_text(&value))
                    .build();
                row.add_css_class("property");
                list.append(&row);
            }
            if let Some(error) = &fm.error {
                let row = adw::ActionRow::builder()
                    .title("Invalid frontmatter")
                    .subtitle(glib::markup_escape_text(error))
                    .build();
                list.append(&row);
            }
        }
        list.upcast()
    }

    // --- per-frame work ------------------------------------------------------

    /// Byte range of the text on screen, extended by `margin` pixels.
    fn visible_bytes(&self, lines: &[usize], margin: i32) -> Range<usize> {
        let rect = self.view.visible_rect();
        let top = self
            .view
            .iter_at_location(0, (rect.y() - margin).max(0))
            .unwrap_or_else(|| self.buffer.start_iter());
        let mut bottom = self
            .view
            .iter_at_location(0, rect.y() + rect.height() + margin)
            .unwrap_or_else(|| self.buffer.end_iter());
        bottom.forward_to_line_end();
        byte_of(lines, &top)..byte_of(lines, &bottom)
    }

    fn tick(&self) {
        let view = &self.view;
        let width = view.width();
        if width > 0 {
            let text_width = (width - 48).clamp(200, 720);
            let margin = ((width - text_width) / 2).max(24);
            if view.left_margin() != margin {
                view.set_left_margin(margin);
                view.set_right_margin(margin);
            }
        }
        let Ok(st) = self.state.try_borrow() else {
            return;
        };
        let left = view.left_margin();
        let fm_end = st.doc.frontmatter.as_ref().map(|f| f.range.end);
        let visible = self.visible_bytes(&st.lines, view.visible_rect().height());
        for overlay in &st.overlays {
            let iter = self.buffer.iter_at_mark(&overlay.mark);
            let anchor = byte_of(&st.lines, &iter);
            let near = match overlay.kind {
                OverlayKind::Properties => visible.start <= fm_end.unwrap_or(0),
                _ => visible.contains(&anchor),
            };
            if near && !overlay.attached.get() {
                let widget = overlay
                    .widget
                    .borrow_mut()
                    .get_or_insert_with(|| self.make_widget(overlay, &st.doc))
                    .clone();
                view.add_overlay(&widget, 0, 0);
                st.stats
                    .overlay_attach
                    .set(st.stats.overlay_attach.get() + 1);
                overlay.attached.set(true);
                overlay.last_pos.set((-1, -1));
            } else if !near && overlay.attached.get() {
                if let Some(widget) = overlay.widget.borrow().as_ref() {
                    view.remove(widget);
                }
                overlay.attached.set(false);
                if overlay.kind != OverlayKind::Properties {
                    overlay.widget.replace(None);
                }
            }
            if !near {
                continue;
            }
            let pos = match overlay.kind {
                OverlayKind::Checkbox => {
                    // line_yrange only needs cached line heights; iter_location
                    // would shape the whole line again every frame.
                    let (y, h) = view.line_yrange(&iter);
                    let indent = indent_px(&st.text, anchor);
                    (left + indent - 2, y + (h - overlay.height) / 2)
                }
                OverlayKind::Image(_) => {
                    let (y, h) = view.line_yrange(&iter);
                    (left, y + h - overlay.height - 6)
                }
                OverlayKind::Properties => {
                    let body = self.iter_at(&st.lines, fm_end.unwrap_or(0));
                    let (y, _) = view.line_yrange(&body);
                    (left, y + 4)
                }
            };
            if overlay.last_pos.get() != pos
                && let Some(widget) = overlay.widget.borrow().as_ref()
            {
                view.move_overlay(widget, pos.0, pos.1);
                st.stats.overlay_moves.set(st.stats.overlay_moves.get() + 1);
                overlay.last_pos.set(pos);
            }
        }
    }

    fn draw_decor(&self, view: &LpView, snapshot: &gtk::Snapshot) {
        let Ok(st) = self.state.try_borrow() else {
            return;
        };
        if st.mode == Mode::Source || off("decor") {
            return;
        }
        let visible = view.visible_rect();
        let left = view.left_margin() as f32;
        let width = (view.width() - view.left_margin() - view.right_margin()) as f32;
        let fg = &self.palette.fg;
        let accent = &self.palette.accent;
        let block = |range: &Range<usize>| -> Option<graphene::Rect> {
            let (y0, _) = view.line_yrange(&self.iter_at(&st.lines, range.start));
            let (y1, h1) = view.line_yrange(&self.iter_at(&st.lines, range.end));
            let (top, bottom) = (y0, y1 + h1);
            if bottom < visible.y() || top > visible.y() + visible.height() {
                return None;
            }
            Some(graphene::Rect::new(
                left - 10.0,
                top as f32 - 2.0,
                width + 20.0,
                (bottom - top) as f32 + 2.0,
            ))
        };
        let revealed =
            |span: &StyledSpan| st.mode == Mode::Live && span.reveal.is_revealed_by(&st.selection);
        let on_screen = self.visible_bytes(&st.lines, 0);
        let spans = st
            .spans
            .iter()
            .filter(|s| s.range.start <= on_screen.end && s.range.end >= on_screen.start);
        for span in spans {
            match &span.style {
                Style::CodeBlock { .. } => {
                    if let Some(rect) = block(&span.range) {
                        rounded(snapshot, &rect, 6.0, &Palette::with_alpha(fg, 0.06));
                    }
                }
                Style::Callout { .. } => {
                    if let Some(rect) = block(&span.range) {
                        rounded(snapshot, &rect, 6.0, &Palette::with_alpha(accent, 0.10));
                        let bar = graphene::Rect::new(rect.x(), rect.y(), 3.0, rect.height());
                        snapshot.append_color(accent, &bar);
                    }
                }
                Style::Quote => {
                    if let Some(rect) = block(&span.range) {
                        let bar = graphene::Rect::new(rect.x(), rect.y(), 3.0, rect.height());
                        snapshot.append_color(&Palette::with_alpha(fg, 0.3), &bar);
                    }
                }
                Style::ListBullet if !revealed(span) => {
                    let (y, h) = view.line_yrange(&self.iter_at(&st.lines, span.range.start));
                    let indent = indent_px(&st.text, span.range.start) as f32;
                    let (cx, cy) = (left + indent + 4.0, y as f32 + h as f32 / 2.0 - 1.0);
                    let dot = graphene::Rect::new(cx - 2.5, cy - 2.5, 5.0, 5.0);
                    rounded(snapshot, &dot, 2.5, &Palette::with_alpha(fg, 0.7));
                }
                Style::Rule if !revealed(span) => {
                    let (y, h) = view.line_yrange(&self.iter_at(&st.lines, span.range.start));
                    let line = graphene::Rect::new(left, y as f32 + h as f32 / 2.0, width, 1.0);
                    snapshot.append_color(&Palette::with_alpha(fg, 0.25), &line);
                }
                _ => {}
            }
        }
    }
}

/// Approximate pixel indent of the list item at `pos` (spike: 28px per level).
fn indent_px(text: &str, pos: usize) -> i32 {
    let line = &text[igneous_markdown::text::line_start(text, pos)..pos];
    let columns: usize = line.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
    (columns / 2) as i32 * 14
}

fn rounded(snapshot: &gtk::Snapshot, rect: &graphene::Rect, radius: f32, color: &gdk::RGBA) {
    let clip = gsk::RoundedRect::from_rect(*rect, radius);
    snapshot.push_rounded_clip(&clip);
    snapshot.append_color(color, rect);
    snapshot.pop();
}

/// Finds an attachment next to the note, or anywhere below its folder.
fn find_file(dir: &Path, target: &str) -> Option<PathBuf> {
    let direct = dir.join(target);
    if direct.is_file() {
        return Some(direct);
    }
    let name = Path::new(target).file_name()?;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).ok()?.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name() == Some(name) {
                return Some(path);
            }
        }
    }
    None
}
