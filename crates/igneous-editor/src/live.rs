//! Live Preview on a GtkSourceView buffer (see docs/decisions/0001-editor.md).
//!
//! - **Restyling.** After every change the note is re-parsed and its
//!   presentation spans recomputed; only the differences in tag coverage are
//!   applied to the buffer.
//! - **Concealment.** Syntax outside the cursor's reach is shrunk to nothing
//!   (scale 0.01, transparent). GTK's `invisible` property is never used: it
//!   crashes GTK and makes the cursor skip text.
//! - **Overlays.** Checkboxes, images, embeds, tables, callout icons and the
//!   properties header are widgets placed over the text with
//!   `gtk_text_view_add_overlay`, with space reserved by line-spacing tags.
//!   They exist only near the visible area. GTK 4.22 can't remove an overlay
//!   (`gtk_text_view_remove` only knows anchored children), so widgets go
//!   into reusable slots that stay on the view and are hidden when empty.
//! - **Decoration.** Code panels, callout panels, quote bars, bullets and
//!   rules are drawn beneath the text.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;

use gtk::{gdk, glib, graphene, gsk, pango, prelude::*, subclass::prelude::*};
use igneous_markdown::present::{self, Replacement, Reveal, Style, StyledSpan};
use igneous_markdown::{LinkKind, LinkRef, parse};
use sourceview::prelude::*;

use crate::rangeset::RangeSet;
use crate::tags::{Palette, TagKind, callout_icon, callout_role};
use crate::view::{Mode, NoteView, State};

/// Tallest an embedded note gets before it scrolls.
const EMBED_MAX_HEIGHT: i32 = 420;
/// How deeply notes may embed notes.
const MAX_EMBED_DEPTH: u8 = 2;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OverlayKind {
    Checkbox,
    Image(String),
    NoteEmbed(LinkRef),
    Table,
    /// Display math, rendered.
    Math,
    /// A ` ```base ` block, shown as the base's results.
    Base,
    CalloutIcon {
        kind: String,
        has_title: bool,
    },
    Properties,
    FoldToggle {
        folded: bool,
    },
    /// The Linked Mentions section after the last line.
    Mentions,
}

impl Overlay {
    pub(crate) fn is_mentions(&self) -> bool {
        self.kind == OverlayKind::Mentions
    }

    pub(crate) fn mark_stale(&self) {
        self.stale.set(true);
    }
}

/// A widget shown over the text. Widgets exist only while near the screen:
/// GTK re-allocates every text view child on each layout, so hundreds of
/// off-screen checkboxes would make every frame slow.
pub(crate) struct Overlay {
    kind: OverlayKind,
    /// Where the element starts.
    mark: gtk::TextMark,
    /// Where it ends (for tables).
    end: usize,
    checked: Cell<bool>,
    widget: RefCell<Option<gtk::Widget>>,
    /// The slot showing the widget, while it's near the visible area.
    slot: RefCell<Option<crate::slot::Slot>>,
    /// Built with old colours or an old host; replaced on the next frame.
    stale: Cell<bool>,
    height: Cell<i32>,
    last_pos: Cell<(i32, i32)>,
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

fn byte_of(lines: &[usize], iter: &gtk::TextIter) -> usize {
    lines.get(iter.line() as usize).copied().unwrap_or(0) + iter.line_index() as usize
}

fn kind_key(kind: TagKind) -> String {
    format!("{kind:?}")
}

impl NoteView {
    pub(crate) fn connect_live_preview(&self, buffer: &sourceview::Buffer) {
        buffer.connect_insert_text(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, iter, text| {
                let mut st = view.imp().state.borrow_mut();
                let pos = byte_of(&st.lines, iter);
                st.pending.push((pos, 0, text.len()));
            }
        ));
        buffer.connect_delete_range(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, start, end| {
                let mut st = view.imp().state.borrow_mut();
                let (a, b) = (byte_of(&st.lines, start), byte_of(&st.lines, end));
                st.pending.push((a.min(b), a.abs_diff(b), 0));
            }
        ));
        buffer.connect_changed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.restyle()
        ));
        buffer.connect_mark_set(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |buffer, _, mark| {
                if mark == &buffer.get_insert() || mark == &buffer.selection_bound() {
                    view.update_reveal();
                }
            }
        ));
        self.add_tick_callback(|view, _| {
            let view = view.downcast_ref::<NoteView>().unwrap();
            if view.width() != view.imp().last_width.get() {
                view.schedule_relayout();
            }
            view.place_overlays();
            glib::ControlFlow::Continue
        });
        self.connect_link_clicks();
    }

    // --- positions -------------------------------------------------------------

    fn iter_at(&self, lines: &[usize], byte: usize) -> gtk::TextIter {
        let buffer = self.buffer();
        let line = lines.partition_point(|&s| s <= byte).saturating_sub(1);
        let index = byte - lines[line];
        buffer
            .iter_at_line_index(line as i32, index as i32)
            .unwrap_or_else(|| buffer.end_iter())
    }

    fn selection_bytes(&self, lines: &[usize]) -> Range<usize> {
        let buffer = self.buffer();
        let a = byte_of(lines, &buffer.iter_at_mark(&buffer.get_insert()));
        let b = byte_of(lines, &buffer.iter_at_mark(&buffer.selection_bound()));
        a.min(b)..a.max(b)
    }

    fn palette(&self) -> Option<Palette> {
        self.imp().palette.borrow().clone()
    }

    // --- modes -----------------------------------------------------------------

    pub fn set_mode(&self, mode: Mode) {
        self.set_editable(mode != Mode::Reading);
        self.set_cursor_visible(mode != Mode::Reading);
        self.imp().state.borrow_mut().mode = mode;
        self.apply_scheme();
        self.restyle_all();
    }

    /// Restyles from scratch, e.g. after a mode change.
    pub(crate) fn restyle_all(&self) {
        {
            let mut st = self.imp().state.borrow_mut();
            // Force the full path: forget what's applied (but not folds).
            st.pending.push((0, 0, 0));
            st.pending.push((0, 0, 0));
            st.fold_kept = true;
        }
        self.restyle();
    }

    // --- restyling -------------------------------------------------------------

    pub(crate) fn restyle(&self) {
        let buffer = self.buffer();
        let (a, b) = buffer.bounds();
        let text = buffer.text(&a, &b, true).to_string();
        let host = self.host();
        {
            let mut guard = self.imp().state.borrow_mut();
            let st = &mut *guard;
            let edits = std::mem::take(&mut st.pending);
            if edits.len() != 1 && st.text != text {
                // Replaced wholesale (a reload): old problems no longer apply.
                st.diagnostics.clear();
            }
            // A load replaces the whole text (into an empty buffer that's a
            // single insert).
            let wholesale = match edits[..] {
                [(0, deleted, inserted)] => deleted == st.text.len() || inserted == text.len(),
                [_] => false,
                _ => true,
            };
            st.text = text;
            st.lines = line_starts(&st.text);
            if let [(pos, deleted, inserted)] = edits[..] {
                crate::fold::shift(&mut st.folded, pos, deleted, inserted);
                // Overlay starts are marks and move by themselves; their ends
                // are offsets and must follow the edit too, or every widget
                // after the cursor would look moved and be rebuilt.
                for overlay in &mut st.overlays {
                    if overlay.end > pos {
                        overlay.end = (overlay.end.max(pos + deleted) - deleted) + inserted;
                    }
                }
                for set in st.applied.values_mut() {
                    set.apply_edit(pos, deleted, inserted);
                }
                crate::diagnostics::shift(&mut st.diagnostics, pos, deleted, inserted);
                if inserted > 0 {
                    // GTK gives inserted text the tags around it; strip ours so
                    // the buffer matches what's recorded.
                    let (from, to) = (
                        self.iter_at(&st.lines, pos),
                        self.iter_at(&st.lines, pos + inserted),
                    );
                    for key in st.applied.keys() {
                        buffer.remove_tag(&self.tag_for(key), &from, &to);
                    }
                }
            } else {
                let (from, to) = buffer.bounds();
                for key in st.applied.keys() {
                    buffer.remove_tag(&self.tag_for(key), &from, &to);
                }
                st.applied.clear();
            }
            st.doc = parse(&st.text);
            st.spans = present::spans(&st.doc, &st.text);
            st.foldables =
                crate::fold::foldables(&st.doc, &st.text, self.imp().fold_headings.get());
            if wholesale && !st.fold_kept {
                st.folded = crate::fold::folded_by_default(&st.doc);
            }
            st.fold_kept = false;
            st.unresolved = st
                .doc
                .links
                .iter()
                .filter(|l| {
                    matches!(l.kind, LinkKind::Wiki | LinkKind::Markdown)
                        && !l.reference.target.is_empty()
                        && !is_external(&l.reference.target)
                        && !host.link_exists(&l.reference)
                })
                .map(|l| l.range.clone())
                .collect();
            st.selection = self.selection_bytes(&st.lines);
            let mut desired = self.style_coverage(st);
            self.add_concealment(st, &mut desired);
            // Tags no longer wanted at all must come off too.
            let mut keys: Vec<String> = desired
                .keys()
                .chain(st.applied.keys())
                .filter(|k| !k.starts_with("space:"))
                .cloned()
                .collect();
            keys.sort_unstable();
            keys.dedup();
            self.apply(st, desired, &keys);
        }
        self.sync_overlays();
        self.queue_draw();
    }

    fn tag_for(&self, key: &str) -> gtk::TextTag {
        if key == crate::diagnostics::KEY {
            return self.tags().diagnostic.clone();
        }
        if let Some(role) = key.strip_prefix("title:") {
            return self
                .tags()
                .callout_title(role.trim_start_matches("callout-"));
        }
        if let Some(name) = key.strip_prefix("space:") {
            return self.tags().spacing(name);
        }
        let kind = TagKind::ALL
            .iter()
            .find(|k| kind_key(**k) == key)
            .expect("known tag key");
        self.tags().get(*kind).clone()
    }

    /// Which text each style tag should cover. Source mode styles nothing:
    /// the language definition highlights it instead.
    fn style_coverage(&self, st: &State) -> HashMap<String, Vec<Range<usize>>> {
        let mut map: HashMap<String, Vec<Range<usize>>> = HashMap::new();
        for kind in TagKind::ALL {
            map.entry(kind_key(kind)).or_default();
        }
        map.insert(
            crate::diagnostics::KEY.to_owned(),
            crate::diagnostics::coverage(&st.diagnostics, st.mode),
        );
        if st.mode == Mode::Source {
            return map;
        }
        for span in &st.spans {
            if let Some(kind) = TagKind::for_style(&span.style) {
                map.entry(kind_key(kind))
                    .or_default()
                    .push(span.range.clone());
            }
            if let Style::Callout { kind } = &span.style {
                // The title (or, without one, the first line) in the
                // callout's colour.
                let first_line_end = igneous_markdown::text::line_end(&st.text, span.range.start);
                map.entry(format!("title:{}", callout_role(kind)))
                    .or_default()
                    .push(span.range.start..first_line_end);
            }
            map.entry(kind_key(TagKind::Syntax))
                .or_default()
                .extend(span.markers.iter().cloned());
        }
        map.entry(kind_key(TagKind::Unresolved))
            .or_default()
            .extend(st.unresolved.iter().cloned());
        map
    }

    /// The tags that depend on where the cursor is: what's hidden, and the
    /// room left for callout icons.
    const REVEAL_KEYS: [TagKind; 3] = [TagKind::Conceal, TagKind::Ghost, TagKind::CalloutHeading];

    fn add_concealment(&self, st: &State, map: &mut HashMap<String, Vec<Range<usize>>>) {
        for kind in Self::REVEAL_KEYS {
            map.entry(kind_key(kind)).or_default();
        }
        if st.mode == Mode::Source {
            return;
        }
        let mut conceal = Vec::new();
        let mut ghost = Vec::new();
        let mut headings = Vec::new();
        let bases = self.host().embeds_bases();
        for span in &st.spans {
            let revealed = st.mode == Mode::Live && span.reveal.is_revealed_by(&st.selection);
            if !revealed {
                match &span.replace {
                    // A base block becomes its results, if the app can show
                    // them; otherwise it stays a code block.
                    Some(Replacement::Base) if bases => conceal.push(span.range.clone()),
                    Some(
                        Replacement::Checkbox { .. } | Replacement::Bullet | Replacement::Rule,
                    ) => ghost.extend(span.markers.iter().cloned()),
                    // Embeds and tables are replaced entirely by their widget.
                    Some(
                        Replacement::Image { .. }
                        | Replacement::NoteEmbed { .. }
                        | Replacement::Table
                        | Replacement::Math,
                    ) => conceal.push(span.range.clone()),
                    // A callout without a title keeps its `[!type]` line's
                    // height (the type is drawn as the title instead).
                    Some(Replacement::CalloutHeader { has_title, .. }) => {
                        let (last, rest) = span.markers.split_last().expect("callout markers");
                        conceal.extend(rest.iter().cloned());
                        if *has_title {
                            conceal.push(last.clone());
                        } else {
                            ghost.push(last.clone());
                        }
                        let first_line_end =
                            igneous_markdown::text::line_end(&st.text, span.range.start);
                        headings.push(span.range.start..first_line_end);
                    }
                    _ => conceal.extend(span.markers.iter().cloned()),
                }
            }
            if st.mode == Mode::Reading && matches!(span.style, Style::Comment | Style::BlockId) {
                conceal.push(span.range.clone());
            }
        }
        for fold in &st.foldables {
            if st.folded.contains(&fold.start) {
                conceal.push(fold.hidden.clone());
            }
        }
        map.entry(kind_key(TagKind::Conceal))
            .or_default()
            .extend(conceal);
        map.entry(kind_key(TagKind::Ghost))
            .or_default()
            .extend(ghost);
        map.entry(kind_key(TagKind::CalloutHeading))
            .or_default()
            .extend(headings);
    }

    pub(crate) fn apply(
        &self,
        st: &mut State,
        mut desired: HashMap<String, Vec<Range<usize>>>,
        keys: &[String],
    ) {
        let buffer = self.buffer();
        let State { lines, applied, .. } = st;
        for key in keys {
            let new = RangeSet::from_ranges(desired.remove(key).unwrap_or_default());
            let old = applied.entry(key.clone()).or_default();
            if *old == new {
                continue;
            }
            let tag = self.tag_for(key);
            for r in old.difference(&new) {
                buffer.remove_tag(
                    &tag,
                    &self.iter_at(lines, r.start),
                    &self.iter_at(lines, r.end),
                );
            }
            for r in new.difference(old) {
                buffer.apply_tag(
                    &tag,
                    &self.iter_at(lines, r.start),
                    &self.iter_at(lines, r.end),
                );
            }
            *old = new;
        }
    }

    fn update_reveal(&self) {
        let (guard_target, widgets_change) = {
            let Ok(mut guard) = self.imp().state.try_borrow_mut() else {
                return;
            };
            let st = &mut *guard;
            if st.mode == Mode::Source {
                return;
            }
            let selection = self.selection_bytes(&st.lines);
            if selection == st.selection {
                return;
            }
            let old = std::mem::replace(&mut st.selection, selection);
            // The cursor went into folded text: unfold it.
            let entered: Vec<usize> = st
                .foldables
                .iter()
                .filter(|f| {
                    st.folded.contains(&f.start)
                        && st.selection.start < f.hidden.end
                        && st.selection.end > f.hidden.start
                })
                .map(|f| f.start)
                .collect();
            for start in &entered {
                st.folded.remove(start);
            }
            // Tables, base blocks and callout icons are widgets only while
            // their source is hidden.
            let widgets_change = st.mode == Mode::Live
                && st.spans.iter().any(|s| {
                    matches!(
                        s.replace,
                        Some(
                            Replacement::Table
                                | Replacement::Math
                                | Replacement::Base
                                | Replacement::CalloutHeader { .. }
                        )
                    ) && s.reveal.is_revealed_by(&old) != s.reveal.is_revealed_by(&st.selection)
                })
                || !entered.is_empty();
            let mut desired = HashMap::new();
            self.add_concealment(st, &mut desired);
            let keys = Self::REVEAL_KEYS.map(kind_key);
            self.apply(st, desired, &keys);
            (self.cursor_guard(st), widgets_change)
        };
        if widgets_change {
            self.sync_overlays();
        }
        self.queue_draw();
        if let Some(target) = guard_target {
            let st = self.imp().state.borrow();
            let iter = self.iter_at(&st.lines, target);
            drop(st);
            self.buffer().place_cursor(&iter);
        }
    }

    /// Keeps the cursor out of text that's hidden and never revealed (the
    /// frontmatter under the properties header).
    fn cursor_guard(&self, st: &State) -> Option<usize> {
        if st.mode != Mode::Live || !st.selection.is_empty() {
            return None;
        }
        let pos = st.selection.start;
        st.spans
            .iter()
            .filter(|s| s.reveal == Reveal::Never)
            .flat_map(|s| s.markers.iter())
            .find(|m| m.start <= pos && pos < m.end)
            .map(|m| m.end)
    }

    // --- overlays ----------------------------------------------------------------

    pub(crate) fn sync_overlays_now(&self) {
        self.sync_overlays();
        self.queue_draw();
    }

    /// Throws away overlay widgets so they're built again (after a theme or
    /// host change).
    pub(crate) fn rebuild_overlays(&self) {
        // Swapped on the next frame, where widgets come and go anyway.
        for overlay in &self.imp().state.borrow().overlays {
            overlay.stale.set(true);
        }
        self.queue_draw();
    }

    /// Shows `widget` over the text, in a free slot or a new one.
    fn take_slot(&self, widget: &gtk::Widget, fit: bool) -> crate::slot::Slot {
        let slot = self.imp().free_slots.borrow_mut().pop().unwrap_or_else(|| {
            let slot = crate::slot::Slot::new();
            self.add_overlay(&slot, 0, 0);
            slot
        });
        slot.set_child(Some(widget), fit);
        slot.set_visible(true);
        slot
    }

    /// Takes an overlay's widget off the view, keeping its slot for reuse.
    fn release_slot(&self, overlay: &Overlay) {
        if let Some(slot) = overlay.slot.take() {
            slot.set_child(None::<&gtk::Widget>, false);
            slot.set_natural_width(0);
            slot.set_visible(false);
            self.imp().free_slots.borrow_mut().push(slot);
        }
    }

    fn sync_overlays(&self) {
        let buffer = self.buffer();
        let desired: Vec<(OverlayKind, usize, usize, bool)> = {
            let st = self.imp().state.borrow();
            let mut desired = Vec::new();
            let bases = self.host().embeds_bases();
            if st.mode != Mode::Source {
                for span in &st.spans {
                    let revealed =
                        st.mode == Mode::Live && span.reveal.is_revealed_by(&st.selection);
                    let start = span.range.start;
                    let kind = match &span.replace {
                        Some(Replacement::Checkbox { checked, .. }) => {
                            desired.push((OverlayKind::Checkbox, start, span.range.end, *checked));
                            continue;
                        }
                        Some(Replacement::Image { target, .. }) => {
                            OverlayKind::Image(target.clone())
                        }
                        Some(Replacement::NoteEmbed { .. }) => {
                            let Some(link) = st.doc.links.iter().find(|l| l.range == span.range)
                            else {
                                continue;
                            };
                            OverlayKind::NoteEmbed(link.reference.clone())
                        }
                        Some(Replacement::Table) if !revealed => OverlayKind::Table,
                        Some(Replacement::Math) if !revealed => OverlayKind::Math,
                        Some(Replacement::Base) if !revealed && bases => OverlayKind::Base,
                        Some(Replacement::CalloutHeader { kind, has_title }) if !revealed => {
                            OverlayKind::CalloutIcon {
                                kind: kind.clone(),
                                has_title: *has_title,
                            }
                        }
                        Some(Replacement::Properties) => OverlayKind::Properties,
                        _ => continue,
                    };
                    desired.push((kind, start, span.range.end, false));
                }
                if st.mentions.is_some() {
                    let end = st.text.len();
                    let start = igneous_markdown::text::line_start(&st.text, end.saturating_sub(1));
                    desired.push((OverlayKind::Mentions, start, end, false));
                }
                for fold in &st.foldables {
                    let folded = st.folded.contains(&fold.start);
                    desired.push((
                        OverlayKind::FoldToggle { folded },
                        fold.start,
                        fold.hidden.end,
                        folded,
                    ));
                }
            }
            desired
        };

        let mut guard = self.imp().state.borrow_mut();
        let st = &mut *guard;
        let fm_source = st
            .doc
            .frontmatter
            .as_ref()
            .map(|f| st.text[f.range.clone()].to_owned())
            .unwrap_or_default();
        let mut keep = vec![false; st.overlays.len()];
        let mut create = Vec::new();
        // Existing widgets by where they are, so matching is one pass (a long
        // note can have hundreds).
        let mut by_place: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, o) in st.overlays.iter().enumerate() {
            let anchor = byte_of(&st.lines, &buffer.iter_at_mark(&o.mark));
            by_place.entry((anchor, o.end)).or_default().push(i);
        }
        for (kind, anchor, end, checked) in desired {
            let found = by_place.get(&(anchor, end)).and_then(|candidates| {
                candidates
                    .iter()
                    .copied()
                    .find(|&i| !keep[i] && st.overlays[i].kind == kind)
            });
            match found {
                Some(_) if kind == OverlayKind::Properties && fm_source != st.properties_source => {
                    create.push((kind, anchor, end, checked));
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
                        self.imp().syncing.set(true);
                        check.set_active(checked);
                        self.imp().syncing.set(false);
                    }
                }
                None => create.push((kind, anchor, end, checked)),
            }
        }
        let mut kept = Vec::new();
        for (overlay, keep) in st.overlays.drain(..).zip(keep) {
            if keep {
                kept.push(overlay);
            } else {
                self.release_slot(&overlay);
                buffer.delete_mark(&overlay.mark);
            }
        }
        st.overlays = kept;
        for (kind, anchor, end, checked) in create {
            let overlay = Overlay {
                kind,
                mark: buffer.create_mark(None, &self.iter_at(&st.lines, anchor), true),
                end,
                checked: Cell::new(checked),
                widget: RefCell::new(None),
                slot: RefCell::new(None),
                stale: Cell::new(false),
                height: Cell::new(0),
                last_pos: Cell::new((-1, -1)),
            };
            // Block widgets are built now: their height decides the space
            // reserved for them.
            if !matches!(
                overlay.kind,
                OverlayKind::Checkbox
                    | OverlayKind::CalloutIcon { .. }
                    | OverlayKind::FoldToggle { .. }
            ) {
                let widget = self.make_widget(&overlay, st);
                overlay
                    .height
                    .set(self.measure_height(&overlay.kind, &widget));
                overlay.widget.replace(Some(widget));
            } else if overlay.kind == OverlayKind::Checkbox {
                overlay.height.set(self.checkbox_height());
            }
            if overlay.kind == OverlayKind::Properties {
                st.properties_source = fm_source.clone();
            }
            st.overlays.push(overlay);
        }
        self.reserve_space(st);
    }

    /// Adds blank space under (or above) the lines that block widgets sit on.
    fn reserve_space(&self, st: &mut State) {
        let buffer = self.buffer();
        let fm_end = st.doc.frontmatter.as_ref().map(|f| f.range.end);
        let mut spacing: HashMap<String, Vec<Range<usize>>> = HashMap::new();
        let text_len = st.text.len();
        for overlay in &st.overlays {
            let anchor = byte_of(&st.lines, &buffer.iter_at_mark(&overlay.mark));
            let height = overlay.height.get();
            let line_of = |pos: usize| {
                let start = igneous_markdown::text::line_start(&st.text, pos);
                let end = (igneous_markdown::text::line_end(&st.text, pos) + 1).min(text_len);
                start..end.max(start + 1).min(text_len.max(start + 1))
            };
            match overlay.kind {
                OverlayKind::Image(_) | OverlayKind::NoteEmbed(_) => {
                    spacing
                        .entry(format!("space:below-{}", height + 8))
                        .or_default()
                        .push(line_of(anchor));
                }
                OverlayKind::Mentions => {
                    spacing
                        .entry(format!("space:below-{}", height + 32))
                        .or_default()
                        .push(line_of(text_len.saturating_sub(1)));
                }
                OverlayKind::Table | OverlayKind::Math | OverlayKind::Base => {
                    let last = overlay.end.saturating_sub(1).max(anchor);
                    spacing
                        .entry(format!("space:below-{}", height + 8))
                        .or_default()
                        .push(line_of(last));
                }
                OverlayKind::Properties => {
                    let body = fm_end.unwrap_or(0);
                    if body < text_len {
                        spacing
                            .entry(format!("space:above-{}", height + 20))
                            .or_default()
                            .push(line_of(body));
                    }
                }
                OverlayKind::Checkbox
                | OverlayKind::CalloutIcon { .. }
                | OverlayKind::FoldToggle { .. } => {}
            }
        }
        let mut keys: Vec<String> = st
            .applied
            .keys()
            .filter(|k| k.starts_with("space:"))
            .cloned()
            .chain(spacing.keys().cloned())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        self.apply(st, spacing, &keys);
    }

    /// The height a block widget needs, laid out as its slot will.
    fn measure_height(&self, kind: &OverlayKind, widget: &gtk::Widget) -> i32 {
        let width =
            crate::slot::layout_width(widget, self.slot_width(kind), Self::fits_content(kind));
        widget.measure(gtk::Orientation::Vertical, width).1
    }

    /// The width a block widget's slot is given: the text column's for most,
    /// none (the widget's own) for the rest.
    fn slot_width(&self, kind: &OverlayKind) -> i32 {
        if Self::spans_text_column(kind) {
            self.text_width()
        } else {
            0
        }
    }

    /// Block widgets that are only as wide as their content, up to the text
    /// column's width.
    fn fits_content(kind: &OverlayKind) -> bool {
        matches!(kind, OverlayKind::Table | OverlayKind::Image(_))
    }

    fn spans_text_column(kind: &OverlayKind) -> bool {
        !matches!(
            kind,
            OverlayKind::Checkbox
                | OverlayKind::CalloutIcon { .. }
                | OverlayKind::Image(_)
                | OverlayKind::FoldToggle { .. }
        )
    }

    /// A rendered formula, cached by its TeX, colour and size.
    fn math_widget(&self, tex: &str) -> gtk::Widget {
        let color = self.palette().map_or(gdk::RGBA::BLACK, |p| p.role("text"));
        let scale = self.scale_factor().max(1);
        // Display math is a little larger than the text around it.
        let px = (self.em_pixels() * 1.5 * f64::from(scale)).round();
        let key = (tex.to_owned(), crate::tags::rgba_hex(&color), px as i32);
        let texture = self
            .imp()
            .formulas
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| crate::math::render(tex, &color, px))
            .clone();
        crate::math::widget(texture, scale, tex)
    }

    /// The text's font size in logical pixels.
    fn em_pixels(&self) -> f64 {
        let description = self.pango_context().font_description();
        let size = description.as_ref().map_or(0, |d| d.size());
        let px = if description.as_ref().is_some_and(|d| d.is_size_absolute()) {
            f64::from(size) / f64::from(pango::SCALE)
        } else {
            f64::from(size) / f64::from(pango::SCALE) * 96.0 / 72.0
        };
        if px > 0.0 { px } else { 14.0 }
    }

    fn fold_toggle_widget(&self, start: usize, folded: bool) -> gtk::Widget {
        let button = gtk::Button::builder()
            .icon_name(if folded {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            })
            .tooltip_text(if folded { "Unfold" } else { "Fold" })
            .css_classes(["flat", "circular", "fold-toggle"])
            .focus_on_click(false)
            .can_focus(false)
            .build();
        if folded {
            button.add_css_class("folded");
        }
        button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.toggle_fold(start)
        ));
        button.upcast()
    }

    /// Folds or unfolds the heading or callout starting at `start`.
    pub fn toggle_fold(&self, start: usize) {
        let move_cursor = {
            let mut guard = self.imp().state.borrow_mut();
            let st = &mut *guard;
            if st.folded.remove(&start) {
                None
            } else {
                st.folded.insert(start);
                // Don't fold the cursor away: move it to the heading.
                st.foldables
                    .iter()
                    .find(|f| f.start == start)
                    .filter(|f| {
                        st.selection.start >= f.hidden.start && st.selection.start <= f.hidden.end
                    })
                    .map(|_| self.iter_at(&st.lines, start))
            }
        };
        if let Some(iter) = move_cursor {
            self.buffer().place_cursor(&iter);
        }
        {
            let mut guard = self.imp().state.borrow_mut();
            let st = &mut *guard;
            let mut desired = HashMap::new();
            self.add_concealment(st, &mut desired);
            let keys = Self::REVEAL_KEYS.map(kind_key);
            self.apply(st, desired, &keys);
        }
        self.sync_overlays();
        self.queue_draw();
    }

    /// The starts of folded headings and callouts. For tests.
    #[doc(hidden)]
    pub fn folded(&self) -> Vec<usize> {
        self.imp().state.borrow().folded.iter().copied().collect()
    }

    fn text_width(&self) -> i32 {
        let width = self.width();
        if width > 0 {
            (width - self.left_margin() - self.right_margin()).max(200)
        } else {
            crate::view::READABLE_WIDTH
        }
    }

    fn checkbox_height(&self) -> i32 {
        let imp = self.imp();
        if imp.checkbox_height.get() == 0 {
            let probe = gtk::CheckButton::new();
            imp.checkbox_height
                .set(probe.measure(gtk::Orientation::Vertical, -1).1);
        }
        imp.checkbox_height.get()
    }

    fn texture(&self, target: &str) -> Option<gdk::Texture> {
        let host = self.host();
        self.imp()
            .textures
            .borrow_mut()
            .entry(target.to_owned())
            .or_insert_with(|| {
                host.image_path(target)
                    .and_then(|path| gdk::Texture::from_filename(path).ok())
            })
            .clone()
    }

    fn make_widget(&self, overlay: &Overlay, st: &State) -> gtk::Widget {
        match &overlay.kind {
            OverlayKind::Checkbox => self.checkbox_widget(overlay.checked.get()),
            OverlayKind::Image(target) => self.image_widget(target, st, overlay),
            OverlayKind::NoteEmbed(link) => self.embed_widget(link),
            OverlayKind::Table => {
                let anchor = byte_of(&st.lines, &self.buffer().iter_at_mark(&overlay.mark));
                self.table_widget(&st.text[anchor..overlay.end.min(st.text.len())])
            }
            OverlayKind::Math => {
                let anchor = byte_of(&st.lines, &self.buffer().iter_at_mark(&overlay.mark));
                let quoted = st.text[igneous_markdown::text::line_start(&st.text, anchor)..anchor]
                    .contains('>');
                let tex =
                    crate::math::source(&st.text[anchor..overlay.end.min(st.text.len())], quoted);
                self.math_widget(&tex)
            }
            OverlayKind::Base => {
                let anchor = byte_of(&st.lines, &self.buffer().iter_at_mark(&overlay.mark));
                let block = &st.text[anchor..overlay.end.min(st.text.len())];
                self.base_widget(crate::BaseEmbed::Block(fence_body(block)))
            }
            OverlayKind::CalloutIcon { kind, has_title } => {
                self.callout_icon_widget(kind, *has_title)
            }
            OverlayKind::Properties => self.properties_widget(st),
            OverlayKind::FoldToggle { folded } => {
                let anchor = byte_of(&st.lines, &self.buffer().iter_at_mark(&overlay.mark));
                self.fold_toggle_widget(anchor, *folded)
            }
            OverlayKind::Mentions => self.mentions_widget(st.mentions.as_deref().unwrap_or(&[])),
        }
    }

    fn mentions_widget(&self, mentions: &[crate::Mention]) -> gtk::Widget {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["linked-mentions"])
            .build();
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(
            &gtk::Label::builder()
                .label(format!("Linked Mentions ({})", mentions.len()))
                .xalign(0.0)
                .margin_top(6)
                .css_classes(["heading"])
                .build(),
        );
        if mentions.is_empty() {
            content.append(
                &gtk::Label::builder()
                    .label("No notes link here yet.")
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        for mention in mentions {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&mention.title))
                .subtitle(glib::markup_escape_text(mention.line.trim()))
                .subtitle_lines(2)
                .activatable(true)
                .tooltip_text(&mention.path)
                .build();
            let mention = mention.clone();
            adw::prelude::ActionRowExt::connect_activated(
                &row,
                glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    move |_| view.host().open_mention(&mention)
                ),
            );
            list.append(&row);
        }
        if !mentions.is_empty() {
            content.append(&list);
        }
        content.upcast()
    }

    fn checkbox_widget(&self, checked: bool) -> gtk::Widget {
        let check = gtk::CheckButton::new();
        check.set_active(checked);
        check.set_focus_on_click(false);
        check.set_can_focus(false);
        check.set_tooltip_text(Some(if checked {
            "Mark as not done"
        } else {
            "Mark as done"
        }));
        check.connect_toggled(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |check| {
                if !view.imp().syncing.get() {
                    view.toggle_task_at(check.upcast_ref());
                }
            }
        ));
        check.upcast()
    }

    fn toggle_task_at(&self, widget: &gtk::Widget) {
        let buffer = self.buffer();
        let edit = {
            let st = self.imp().state.borrow();
            let Some(overlay) = st
                .overlays
                .iter()
                .find(|o| o.widget.borrow().as_ref() == Some(widget))
            else {
                return;
            };
            let anchor = byte_of(&st.lines, &buffer.iter_at_mark(&overlay.mark));
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
            buffer.begin_user_action();
            let mut a = buffer.iter_at_offset(from);
            let mut b = buffer.iter_at_offset(to);
            buffer.delete(&mut a, &mut b);
            let mut at = buffer.iter_at_offset(from);
            buffer.insert(&mut at, replacement);
            buffer.end_user_action();
        }
    }

    fn image_widget(&self, target: &str, st: &State, overlay: &Overlay) -> gtk::Widget {
        let Some(texture) = self.texture(target) else {
            let label = gtk::Label::new(Some(&format!("Missing image: {target}")));
            label.add_css_class("dim-label");
            label.set_xalign(0.0);
            return label.upcast();
        };
        // `![[image.png|300]]` and `|300x200` set the display width.
        let anchor = byte_of(&st.lines, &self.buffer().iter_at_mark(&overlay.mark));
        let size = st
            .doc
            .links
            .iter()
            .find(|l| l.range.start == anchor)
            .and_then(|l| l.reference.size);
        let (w, h) = (texture.width() as f64, texture.height() as f64);
        let max = f64::from(self.text_width());
        let width = match size {
            Some((width, _)) => f64::from(width).min(max),
            None => w.min(max),
        };
        let height = match size {
            Some((_, Some(height))) => f64::from(height),
            _ => h * width / w.max(1.0),
        };
        let picture = gtk::Picture::for_paintable(&texture);
        picture.set_size_request(width as i32, height.round() as i32);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_halign(gtk::Align::Start);
        picture.set_alternative_text(Some(target));
        // The frame rounds the picture's corners.
        let frame = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["md-image"])
            .build();
        frame.append(&picture);
        frame.upcast()
    }

    /// A base's results in a card, or a note saying they can't be shown.
    fn base_widget(&self, base: crate::BaseEmbed<'_>) -> gtk::Widget {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
        card.add_css_class("card");
        card.add_css_class("base-embed-card");
        match self.host().base_widget(base) {
            Some(widget) => {
                widget.set_margin_start(8);
                widget.set_margin_end(8);
                widget.set_margin_top(8);
                widget.set_margin_bottom(8);
                card.append(&widget);
            }
            None => {
                let label = gtk::Label::new(Some("This base can’t be shown here"));
                label.add_css_class("dim-label");
                label.set_margin_top(12);
                label.set_margin_bottom(12);
                card.append(&label);
            }
        }
        card.upcast()
    }

    fn embed_widget(&self, link: &LinkRef) -> gtk::Widget {
        let host = self.host();
        if host.embeds_bases()
            && link
                .target
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("base"))
        {
            return self.base_widget(crate::BaseEmbed::File(link));
        }
        let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
        card.add_css_class("card");
        card.add_css_class("note-embed");
        let Some(embed) = host.embed(link) else {
            let label = gtk::Label::new(Some(&format!("“{}” doesn’t exist yet", link.target)));
            label.add_css_class("dim-label");
            label.set_margin_top(12);
            label.set_margin_bottom(12);
            card.append(&label);
            return card.upcast();
        };
        let title = gtk::Label::builder()
            .label(&embed.title)
            .xalign(0.0)
            .css_classes(["heading"])
            .margin_start(12)
            .margin_top(8)
            .build();
        card.append(&title);
        if self.depth() >= MAX_EMBED_DEPTH {
            return card.upcast();
        }
        // The embedded note, read-only, rendered by a nested view.
        let inner = NoteView::new();
        inner.set_depth(self.depth() + 1);
        inner.imp().host.replace(self.imp().host.borrow().clone());
        if let Some(palette) = self.palette() {
            inner.tags().set_palette(&palette);
            inner.imp().palette.replace(Some(palette));
        }
        inner
            .source_buffer()
            .set_style_scheme(self.source_buffer().style_scheme().as_ref());
        if let Some(family) = self.imp().monospace.borrow().as_deref() {
            inner.set_monospace_family(Some(family));
        }
        inner.set_top_margin(4);
        inner.set_bottom_margin(8);
        inner.add_css_class("embedded");
        inner.source_buffer().set_text(&embed.text);
        inner.set_mode(Mode::Reading);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(EMBED_MAX_HEIGHT)
            .child(&inner)
            .build();
        card.append(&scrolled);
        // Clicking the title opens the embedded note.
        let click = gtk::GestureClick::new();
        let target = link.clone();
        click.connect_released(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |gesture, _, _, _| {
                let new_tab = gesture.current_button() == 2;
                view.host().open_link(&target, new_tab);
            }
        ));
        title.add_controller(click);
        title.set_cursor_from_name(Some("pointer"));
        card.upcast()
    }

    fn table_widget(&self, source: &str) -> gtk::Widget {
        let highlight = self
            .palette()
            .map(|p| crate::tags::rgba_hex(&p.role("highlight")))
            .unwrap_or_else(|| "#ffff0066".to_owned());
        let rows: Vec<Vec<String>> = source
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(split_row)
            .collect();
        let aligns: Vec<f32> = rows
            .get(1)
            .map(|delims| {
                delims
                    .iter()
                    .map(|d| {
                        let d = d.trim();
                        match (d.starts_with(':'), d.ends_with(':')) {
                            (true, true) => 0.5,
                            (false, true) => 1.0,
                            _ => 0.0,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let grid = gtk::Grid::builder()
            .css_classes(["md-table"])
            .overflow(gtk::Overflow::Hidden)
            .build();
        for (r, row) in rows.iter().enumerate().filter(|(r, _)| *r != 1) {
            let grid_row = if r == 0 { 0 } else { r as i32 - 1 };
            for (c, cell) in row.iter().enumerate() {
                let label = gtk::Label::builder()
                    .use_markup(true)
                    .label(crate::markup::inline(cell.trim(), &highlight))
                    .xalign(aligns.get(c).copied().unwrap_or(0.0))
                    .wrap(true)
                    .wrap_mode(pango::WrapMode::WordChar)
                    .width_chars(cell.trim().chars().count().clamp(4, 24) as i32)
                    .hexpand(true)
                    .css_classes(if r == 0 {
                        vec!["md-table-head"]
                    } else {
                        vec![]
                    })
                    .build();
                let cell_box = gtk::Box::builder().css_classes(["md-table-cell"]).build();
                cell_box.append(&label);
                grid.attach(&cell_box, c as i32, grid_row, 1, 1);
            }
        }
        grid.upcast()
    }

    fn callout_icon_widget(&self, kind: &str, has_title: bool) -> gtk::Widget {
        let colour = self
            .palette()
            .map(|p| crate::tags::rgba_hex(&p.role(callout_role(kind))));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_can_target(false);
        let image = gtk::Image::from_icon_name(callout_icon(kind));
        row.append(&image);
        if !has_title {
            let mut title = kind.to_owned();
            if let Some(first) = title.get(..1) {
                title = first.to_uppercase() + &title[1..];
            }
            let label = gtk::Label::new(None);
            label.set_markup(&match &colour {
                Some(c) => format!(
                    "<span foreground=\"{c}\" weight=\"bold\">{}</span>",
                    glib::markup_escape_text(&title)
                ),
                None => format!("<b>{}</b>", glib::markup_escape_text(&title)),
            });
            row.append(&label);
        }
        if let Some(c) = colour {
            // Symbolic icons take the widget's CSS colour.
            let css = gtk::CssProvider::new();
            css.load_from_string(&format!("image {{ color: {c}; }}"));
            #[allow(deprecated)]
            image
                .style_context()
                .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        row.upcast()
    }

    fn properties_widget(&self, st: &State) -> gtk::Widget {
        match &st.doc.frontmatter {
            Some(fm) => self.properties_widget_for(fm),
            None => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        }
    }

    // --- per-frame work -----------------------------------------------------------

    /// Byte range of the text on screen, extended by `margin` pixels.
    fn visible_bytes(&self, lines: &[usize], margin: i32) -> Range<usize> {
        let buffer = self.buffer();
        let rect = self.visible_rect();
        let top = self
            .iter_at_location(0, (rect.y() - margin).max(0))
            .unwrap_or_else(|| buffer.start_iter());
        let mut bottom = self
            .iter_at_location(0, rect.y() + rect.height() + margin)
            .unwrap_or_else(|| buffer.end_iter());
        bottom.forward_to_line_end();
        byte_of(lines, &top)..byte_of(lines, &bottom)
    }

    /// Attaches widgets near the visible area, detaches the rest, and moves
    /// them to follow their lines.
    fn place_overlays(&self) {
        if self.place_overlays_inner() {
            // A widget's height changed (text views lay out lazily): move the
            // text below it, between frames.
            self.schedule_relayout();
        }
    }

    /// Updates the margins and the space reserved for block widgets before
    /// the next frame. Doing either during a frame changes the text's
    /// layout while GTK is laying the window out, and GTK then skips
    /// drawing the note for that frame.
    fn schedule_relayout(&self) {
        if self.imp().relayout_pending.replace(true) {
            return;
        }
        // Before GTK's own text validation (GTK_PRIORITY_RESIZE - 2) and the
        // next frame.
        glib::idle_add_local_full(
            glib::Priority::HIGH_IDLE,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    view.imp().relayout_pending.set(false);
                    view.update_margins();
                    if let Ok(mut st) = view.imp().state.try_borrow_mut() {
                        view.reserve_space(&mut st);
                    }
                    glib::ControlFlow::Break
                }
            ),
        );
    }

    /// Returns whether any block widget's height changed.
    fn place_overlays_inner(&self) -> bool {
        let Ok(st) = self.imp().state.try_borrow() else {
            return false;
        };
        if st.overlays.is_empty() {
            return false;
        }
        let mut resized = false;
        let buffer = self.buffer();
        let left = self.left_margin();
        let fm_end = st.doc.frontmatter.as_ref().map(|f| f.range.end);
        let visible = self.visible_bytes(&st.lines, self.visible_rect().height());
        for overlay in &st.overlays {
            let iter = buffer.iter_at_mark(&overlay.mark);
            let anchor = byte_of(&st.lines, &iter);
            let near = match overlay.kind {
                OverlayKind::Properties => visible.start <= fm_end.unwrap_or(0),
                OverlayKind::Table | OverlayKind::Math | OverlayKind::Base => {
                    anchor <= visible.end && overlay.end >= visible.start
                }
                _ => visible.contains(&anchor),
            };
            if overlay.stale.take() {
                self.release_slot(overlay);
                let widget = self.make_widget(overlay, &st);
                overlay.widget.replace(Some(widget));
                overlay.last_pos.set((-1, -1));
            }
            let attached = overlay.slot.borrow().is_some();
            if near && !attached {
                let widget = overlay
                    .widget
                    .borrow_mut()
                    .get_or_insert_with(|| self.make_widget(overlay, &st))
                    .clone();
                overlay.slot.replace(Some(
                    self.take_slot(&widget, Self::fits_content(&overlay.kind)),
                ));
                overlay.last_pos.set((-1, -1));
            } else if !near && attached {
                self.release_slot(overlay);
                // Small widgets are cheap to make again; keep the rest.
                if matches!(
                    overlay.kind,
                    OverlayKind::Checkbox
                        | OverlayKind::CalloutIcon { .. }
                        | OverlayKind::FoldToggle { .. }
                ) {
                    overlay.widget.replace(None);
                }
            }
            if !near {
                continue;
            }
            if !matches!(
                overlay.kind,
                OverlayKind::Checkbox
                    | OverlayKind::CalloutIcon { .. }
                    | OverlayKind::FoldToggle { .. }
            ) && let Some(slot) = overlay.slot.borrow().as_ref()
            {
                slot.set_natural_width(self.slot_width(&overlay.kind));
                let height = slot.measure(gtk::Orientation::Vertical, -1).1;
                if height != overlay.height.get() {
                    overlay.height.set(height);
                    overlay.last_pos.set((-1, -1));
                    resized = true;
                }
            }
            // line_yrange only needs cached line heights; iter_location would
            // shape the line again every frame.
            let pos = match &overlay.kind {
                OverlayKind::Checkbox => {
                    let (y, h) = self.line_yrange(&iter);
                    let indent = self.indent_px(&st.text, anchor);
                    (left + indent - 2, y + (h - overlay.height.get()) / 2)
                }
                OverlayKind::Image(_) | OverlayKind::NoteEmbed(_) => {
                    let (y, h) = self.line_yrange(&iter);
                    (left, y + h - overlay.height.get() - 4)
                }
                OverlayKind::Table | OverlayKind::Math | OverlayKind::Base => {
                    let (y, _) = self.line_yrange(&iter);
                    (left, y + 2)
                }
                OverlayKind::CalloutIcon { .. } => {
                    let (y, h) = self.line_yrange(&iter);
                    (left + 14, y + (h - 16) / 2 - 1)
                }
                OverlayKind::Mentions => {
                    // In the space reserved under the last line.
                    let (y, h) = self.line_yrange(&iter);
                    (left, y + h - overlay.height.get() - 8)
                }
                OverlayKind::FoldToggle { .. } => {
                    // In the margin, level with the line's text (below any
                    // space reserved above it).
                    let (y, h) = self.line_yrange(&iter);
                    let below = self.pixels_below_lines();
                    let text = (h - below).min(40);
                    ((left - 28).max(0), y + h - below - text + (text - 24) / 2)
                }
                OverlayKind::Properties => {
                    let body = self.iter_at(&st.lines, fm_end.unwrap_or(0));
                    let (y, _) = self.line_yrange(&body);
                    (left, y + 4)
                }
            };
            if overlay.last_pos.get() != pos
                && let Some(slot) = overlay.slot.borrow().as_ref()
            {
                self.move_overlay(slot, pos.0, pos.1);
                overlay.last_pos.set(pos);
            }
        }
        resized
    }

    pub(crate) fn draw_decor(&self, snapshot: &gtk::Snapshot) {
        let Ok(st) = self.imp().state.try_borrow() else {
            return;
        };
        if st.mode == Mode::Source {
            return;
        }
        let Some(palette) = self.palette() else {
            return;
        };
        let visible = self.visible_rect();
        let left = self.left_margin() as f32;
        let width = (self.width() - self.left_margin() - self.right_margin()) as f32;
        let block = |range: &Range<usize>| -> Option<graphene::Rect> {
            let (y0, _) = self.line_yrange(&self.iter_at(&st.lines, range.start));
            let (y1, h1) = self.line_yrange(&self.iter_at(&st.lines, range.end));
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
        let bases = self.host().embeds_bases();
        let spans: Vec<&StyledSpan> = spans.collect();
        // Math in a callout or quote sits on its background instead.
        let quoted = |range: &Range<usize>| {
            spans.iter().any(|s| {
                matches!(s.style, Style::Callout { .. } | Style::Quote)
                    && s.range.start <= range.start
                    && range.end <= s.range.end
            })
        };
        for span in spans.iter().copied() {
            match &span.style {
                Style::Math { display: true } if quoted(&span.range) => {}
                // A base block showing its results has its own card.
                Style::CodeBlock { .. }
                    if bases && span.replace == Some(Replacement::Base) && !revealed(span) => {}
                Style::CodeBlock { .. } | Style::Math { display: true } => {
                    if let Some(rect) = block(&span.range) {
                        rounded(snapshot, &rect, BLOCK_RADIUS, &palette.code_background);
                    }
                }
                Style::Callout { kind } => {
                    if let Some(rect) = block(&span.range) {
                        let colour = palette.role(callout_role(kind));
                        // The bar follows the block's rounded corners.
                        let clip = gsk::RoundedRect::from_rect(rect, BLOCK_RADIUS);
                        snapshot.push_rounded_clip(&clip);
                        snapshot.append_color(&Palette::with_alpha(&colour, 0.1), &rect);
                        let bar = graphene::Rect::new(rect.x(), rect.y(), 3.0, rect.height());
                        snapshot.append_color(&colour, &bar);
                        snapshot.pop();
                    }
                }
                Style::Quote => {
                    if let Some(rect) = block(&span.range) {
                        let bar = graphene::Rect::new(left, rect.y(), 3.0, rect.height());
                        snapshot.append_color(&palette.quote, &bar);
                    }
                }
                Style::ListBullet if !revealed(span) => {
                    let (y, h) = self.line_yrange(&self.iter_at(&st.lines, span.range.start));
                    let indent = self.indent_px(&st.text, span.range.start) as f32;
                    let (cx, cy) = (left + indent + 4.0, y as f32 + h as f32 / 2.0 - 1.0);
                    let dot = graphene::Rect::new(cx - 2.5, cy - 2.5, 5.0, 5.0);
                    rounded(snapshot, &dot, 2.5, &palette.list_marker);
                }
                Style::Rule if !revealed(span) => {
                    let (y, h) = self.line_yrange(&self.iter_at(&st.lines, span.range.start));
                    let line = graphene::Rect::new(left, y as f32 + h as f32 / 2.0, width, 1.0);
                    snapshot.append_color(&Palette::with_alpha(&palette.syntax, 0.5), &line);
                }
                _ => {}
            }
        }
    }

    // --- links -------------------------------------------------------------------

    /// Ctrl+click opens a link, middle-click opens it in a new tab, and in
    /// Reading mode a plain click opens it.
    fn connect_link_clicks(&self) {
        let click = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |gesture, _, x, y| {
                let button = gesture.current_button();
                let ctrl = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                let reading = view.mode() == Mode::Reading;
                let wanted = match button {
                    1 => ctrl || reading,
                    2 => true,
                    _ => false,
                };
                if !wanted || view.mode() == Mode::Source && button == 1 && !ctrl {
                    return;
                }
                if let Some(link) = view.link_at(x, y) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    view.host().open_link(&link, button == 2);
                }
            }
        ));
        self.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |controller, x, y| {
                let ctrl = controller
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                let pointer =
                    (ctrl || view.mode() == Mode::Reading) && view.link_at(x, y).is_some();
                view.set_cursor_from_name(Some(if pointer { "pointer" } else { "text" }));
            }
        ));
        self.add_controller(motion);
    }

    /// A preview of the note linked at `iter`, for hovering.
    pub(crate) fn preview_at(&self, iter: &gtk::TextIter) -> Option<gtk::Widget> {
        let link = {
            let st = self.imp().state.try_borrow().ok()?;
            let pos = byte_of(&st.lines, iter);
            st.doc
                .links
                .iter()
                .find(|l| {
                    l.range.start <= pos
                        && pos < l.range.end
                        && matches!(l.kind, LinkKind::Wiki | LinkKind::Markdown)
                })
                .map(|l| l.reference.clone())?
        };
        self.host().embed(&link)?;
        let card = self.embed_widget(&link);
        card.set_size_request(420, -1);
        Some(card)
    }

    /// The width of the whitespace before `pos` on its line, as laid out
    /// (with the view's tab stops), so bullets and checkboxes sit exactly
    /// where their markers are. Cached per indent.
    fn indent_px(&self, text: &str, pos: usize) -> i32 {
        let start = igneous_markdown::text::line_start(text, pos);
        let indent = &text[start..pos];
        let indent = &indent[..indent.len() - indent.trim_start_matches([' ', '\t']).len()];
        if indent.is_empty() {
            return 0;
        }
        if let Some(px) = self.imp().indents.borrow().get(indent) {
            return *px;
        }
        let layout = self.create_pango_layout(Some(indent));
        layout.set_tabs(self.tabs().as_ref());
        let px = layout.pixel_size().0;
        self.imp()
            .indents
            .borrow_mut()
            .insert(indent.to_owned(), px);
        px
    }

    /// The link under widget coordinates `(x, y)`.
    fn link_at(&self, x: f64, y: f64) -> Option<LinkRef> {
        let (bx, by) =
            self.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.iter_at_location(bx, by)?;
        let st = self.imp().state.try_borrow().ok()?;
        let pos = byte_of(&st.lines, &iter);
        st.doc
            .links
            .iter()
            .find(|l| l.range.start <= pos && pos < l.range.end)
            .map(|l| l.reference.clone())
    }
}

fn is_external(target: &str) -> bool {
    target.contains("://") || target.starts_with("mailto:")
}

/// The lines between a fenced block's opening and closing fences.
fn fence_body(block: &str) -> &str {
    let after_open = block.find('\n').map_or(block.len(), |i| i + 1);
    let body = &block[after_open..];
    let trimmed = body.trim_end_matches(['\n', '\r']);
    match trimmed.rfind('\n') {
        Some(i) if trimmed[i + 1..].trim_start().starts_with(['`', '~']) => &body[..i + 1],
        None if trimmed.trim_start().starts_with(['`', '~']) => "",
        _ => body,
    }
}

/// Splits a table row into cells, honouring escaped pipes.
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut cells = vec![String::new()];
    let mut chars = line.chars().peekable();
    let mut code = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cells.last_mut().unwrap().push('|');
                chars.next();
            }
            '`' => {
                code = !code;
                cells.last_mut().unwrap().push(c);
            }
            '|' if !code => cells.push(String::new()),
            _ => cells.last_mut().unwrap().push(c),
        }
    }
    cells
}

/// The corner radius of every block: code, math, callouts, tables, images
/// and embeds (the widgets' radius is in `igneous.css`).
const BLOCK_RADIUS: f32 = 8.0;

fn rounded(snapshot: &gtk::Snapshot, rect: &graphene::Rect, radius: f32, color: &gdk::RGBA) {
    let clip = gsk::RoundedRect::from_rect(*rect, radius);
    snapshot.push_rounded_clip(&clip);
    snapshot.append_color(color, rect);
    snapshot.pop();
}

impl NoteView {
    /// The kinds of widget Live Preview has placed, in order. For tests.
    #[doc(hidden)]
    pub fn overlay_kinds(&self) -> Vec<String> {
        self.imp()
            .state
            .borrow()
            .overlays
            .iter()
            .map(|o| match &o.kind {
                OverlayKind::Checkbox => "checkbox".to_owned(),
                OverlayKind::Image(t) => format!("image:{t}"),
                OverlayKind::NoteEmbed(l) => format!("embed:{}", l.target),
                OverlayKind::Table => "table".to_owned(),
                OverlayKind::Math => "math".to_owned(),
                OverlayKind::Base => "base".to_owned(),
                OverlayKind::CalloutIcon { kind, .. } => format!("callout:{kind}"),
                OverlayKind::Properties => "properties".to_owned(),
                OverlayKind::FoldToggle { folded } => format!("fold:{folded}"),
                OverlayKind::Mentions => "mentions".to_owned(),
            })
            .collect()
    }

    /// Byte ranges styled as unresolved links. For tests.
    #[doc(hidden)]
    pub fn unresolved_ranges(&self) -> Vec<Range<usize>> {
        self.imp().state.borrow().unresolved.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    const NOTE: &str = "---\ntags: [a]\n---\n# Title\n\nSome **bold** and *it* with [[Link|alias]] and [[Missing]].\n\n- [ ] task\n- [x] done\n- bullet\n\n> [!tip] Tip title\n> body\n\n> [!note]\n> no title\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\n%%hidden%% ^block\n";

    fn view(text: &str) -> NoteView {
        let view = NoteView::new();
        view.source_buffer().set_text(text);
        view
    }

    fn place(view: &NoteView, byte: usize) {
        let text = view.model_text();
        let offset = text[..byte].chars().count() as i32;
        let buffer = view.buffer();
        buffer.place_cursor(&buffer.iter_at_offset(offset));
    }

    fn concealed_text(view: &NoteView) -> Vec<String> {
        let text = view.model_text();
        view.concealed()
            .into_iter()
            .map(|r| text[r].to_owned())
            .collect()
    }

    #[gtk::test]
    fn reveal_follows_the_cursor() {
        let text = "aa **bold** cc\n";
        let view = view(text);
        place(&view, 0);
        assert_eq!(concealed_text(&view), ["**", "**"]);
        place(&view, 5);
        assert!(concealed_text(&view).is_empty());
        place(&view, 13);
        assert_eq!(concealed_text(&view), ["**", "**"]);
        view.check_invariants().unwrap();
    }

    #[gtk::test]
    fn modes() {
        let view = view(NOTE);
        place(&view, NOTE.len());
        assert!(!view.concealed().is_empty());
        let live_widgets = view.overlay_kinds();
        assert!(live_widgets.contains(&"properties".to_owned()));
        assert!(live_widgets.contains(&"table".to_owned()));
        assert_eq!(live_widgets.iter().filter(|k| *k == "checkbox").count(), 2);
        assert!(live_widgets.contains(&"callout:tip".to_owned()));
        assert!(live_widgets.contains(&"callout:note".to_owned()));

        view.set_mode(Mode::Source);
        assert!(view.concealed().is_empty());
        assert!(view.overlay_kinds().is_empty());

        view.set_mode(Mode::Reading);
        // Comments and block IDs disappear in Reading mode.
        let hidden = concealed_text(&view).join("|");
        assert!(hidden.contains("%%hidden%%"), "{hidden}");
        assert!(hidden.contains("^block"), "{hidden}");
        assert!(!view.is_editable());
        view.check_invariants().unwrap();
    }

    #[gtk::test]
    fn frontmatter_keeps_the_cursor_out() {
        let view = view(NOTE);
        place(&view, 4);
        let buffer = view.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert()).offset() as usize;
        assert!(
            cursor >= "---\ntags: [a]\n---\n".chars().count() - 1,
            "{cursor}"
        );
    }

    struct MissingHost;
    impl crate::Host for MissingHost {
        fn link_exists(&self, link: &LinkRef) -> bool {
            link.target != "Missing"
        }
    }

    #[gtk::test]
    fn unresolved_links() {
        let view = view(NOTE);
        assert!(view.unresolved_ranges().is_empty());
        view.set_host(Rc::new(MissingHost));
        let ranges = view.unresolved_ranges();
        assert_eq!(ranges.len(), 1);
        assert_eq!(&NOTE[ranges[0].clone()], "[[Missing]]");
    }

    #[gtk::test]
    fn tasks_toggle_as_undoable_edits() {
        let view = view("- [ ] task\n");
        view.set_mode(Mode::Live);
        let st = view.imp().state.borrow();
        let overlay = &st.overlays[0];
        let widget = view.make_widget(overlay, &st);
        overlay.widget.replace(Some(widget.clone()));
        drop(st);
        widget
            .downcast_ref::<gtk::CheckButton>()
            .unwrap()
            .set_active(true);
        assert_eq!(view.model_text(), "- [x] task\n");
        view.source_buffer().undo();
        assert_eq!(view.model_text(), "- [ ] task\n");
        view.check_invariants().unwrap();
    }

    /// Random edits, cursor moves and mode switches never let the buffer and
    /// Live Preview's model drift apart.
    #[gtk::test]
    fn buffer_invariant_under_random_editing() {
        let view = view(NOTE);
        let buffer = view.source_buffer();
        let snippets = [
            "**",
            "[[",
            "]]",
            "\n",
            "# ",
            "- [ ] ",
            "> [!warning]\n> ",
            "|",
            "---\n",
            "`",
            "==",
            "%%",
            "ß漢",
            "\n\n",
            "![[img.png]]",
            "$$",
            "^id ",
        ];
        let mut seed: u64 = 20261004;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % n.max(1)
        };
        for step in 0..1500 {
            let len = buffer.char_count() as usize;
            match next(10) {
                0..=3 => {
                    let mut at = buffer.iter_at_offset(next(len + 1) as i32);
                    buffer.insert(&mut at, snippets[next(snippets.len())]);
                }
                4..=5 if len > 0 => {
                    let a = next(len);
                    let b = (a + next(8) + 1).min(len);
                    let (mut a, mut b) = (
                        buffer.iter_at_offset(a as i32),
                        buffer.iter_at_offset(b as i32),
                    );
                    buffer.delete(&mut a, &mut b);
                }
                6 => {
                    let at = buffer.iter_at_offset(next(len + 1) as i32);
                    buffer.place_cursor(&at);
                }
                7 => {
                    if buffer.can_undo() {
                        buffer.undo();
                    }
                }
                8 => view.set_mode([Mode::Live, Mode::Source, Mode::Reading, Mode::Live][next(4)]),
                _ => {
                    let a = buffer.iter_at_offset(next(len + 1) as i32);
                    let b = buffer.iter_at_offset(next(len + 1) as i32);
                    buffer.select_range(&a, &b);
                }
            }
            if let Err(e) = view.check_invariants() {
                panic!("step {step}: {e}");
            }
            let text = view.model_text();
            for range in view.concealed() {
                assert!(
                    range.end <= text.len(),
                    "step {step}: {range:?} past the end"
                );
            }
        }
    }

    /// Space reserved for a block widget stays put through later layout
    /// passes (a key listed twice once removed it again).
    #[gtk::test]
    async fn reserved_space_survives_relayout() {
        let text = "---\na: 1\nb: 2\n---\n# Heading\nbody\n";
        let view = view(text);
        let window = gtk::Window::builder()
            .default_width(800)
            .default_height(600)
            .child(&gtk::ScrolledWindow::builder().child(&view).build())
            .build();
        window.present();
        for _ in 0..20 {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let heading = view.buffer().iter_at_line(4).unwrap();
        let (_, height) = view.line_yrange(&heading);
        assert!(height > 60, "the heading line has only {height}px");
        window.close();
    }

    /// Overlay widgets come and go through a fixed set of slots, since GTK
    /// can't remove overlays.
    #[gtk::test]
    async fn overlay_slots_are_reused() {
        let text = "- [ ] a\n- [ ] b\n- [ ] c\n";
        let view = view(text);
        let window = gtk::Window::builder()
            .default_width(600)
            .default_height(400)
            .child(&gtk::ScrolledWindow::builder().child(&view).build())
            .build();
        window.present();
        let children = |view: &NoteView| {
            let mut n = 0;
            let mut child = view.first_child();
            while let Some(c) = child {
                let mut inner = c.first_child();
                while let Some(i) = inner {
                    n += 1;
                    inner = i.next_sibling();
                }
                child = c.next_sibling();
            }
            n
        };
        for _ in 0..10 {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let first = children(&view);
        for round in 0..20 {
            // Remove and restore the tasks: every widget is replaced.
            view.source_buffer()
                .set_text(if round % 2 == 0 { "plain\n" } else { text });
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            children(&view) <= first + 1,
            "{} > {first}",
            children(&view)
        );
        window.close();
    }

    /// A host that shows bases as labels naming what it was asked for.
    #[derive(Default)]
    struct Bases {
        asked: std::cell::RefCell<Vec<String>>,
    }
    impl crate::Host for Bases {
        fn embeds_bases(&self) -> bool {
            true
        }
        fn base_widget(&self, base: crate::BaseEmbed<'_>) -> Option<gtk::Widget> {
            let asked = match base {
                crate::BaseEmbed::Block(body) => format!("block:{body}"),
                crate::BaseEmbed::File(link) => format!("file:{}", link.target),
            };
            self.asked.borrow_mut().push(asked.clone());
            Some(gtk::Label::new(Some(&asked)).upcast())
        }
    }

    #[gtk::test]
    fn base_blocks_and_embeds_show_results() {
        let text = "intro\n\n```base\nviews:\n  - type: table\n```\n\n![[Tasks.base]]\n\nafter\n";
        let view = view(text);
        place(&view, 0);
        // Without a host that can show bases, the block stays code.
        assert!(!view.overlay_kinds().contains(&"base".to_owned()));
        assert!(!concealed_text(&view).join("|").contains("views:"));

        let host = Rc::new(Bases::default());
        let view = NoteView::new();
        view.set_host(host.clone());
        view.source_buffer().set_text(text);
        place(&view, 0);
        assert!(view.overlay_kinds().contains(&"base".to_owned()));
        assert!(concealed_text(&view).join("|").contains("views:"));
        // Widgets are built as overlays are created.
        let asked = host.asked.borrow().clone();
        assert!(
            asked.contains(&"block:views:\n  - type: table\n".to_owned()),
            "{asked:?}"
        );
        assert!(asked.contains(&"file:Tasks.base".to_owned()), "{asked:?}");

        // With the cursor inside, the source shows again.
        place(&view, text.find("views").unwrap());
        assert!(!view.overlay_kinds().contains(&"base".to_owned()));
        assert!(!concealed_text(&view).join("|").contains("views:"));
        view.check_invariants().unwrap();
    }

    #[test]
    fn fence_bodies() {
        assert_eq!(fence_body("```base\na: 1\nb: 2\n```"), "a: 1\nb: 2\n");
        assert_eq!(fence_body("```base\n```"), "");
        assert_eq!(fence_body("```base\nunclosed"), "unclosed");
        assert_eq!(fence_body("~~~base\nx\n~~~"), "x\n");
    }

    #[gtk::test]
    fn folding() {
        let text = "# A\none\n## B\ntwo\n\n> [!tip]- Folded\n> inside\n";
        let view = view(text);
        view.set_fold_headings(true);
        place(&view, text.len());
        // The `-` callout starts folded.
        let callout = text.find("> [!tip]").unwrap();
        assert_eq!(view.folded(), [callout]);
        assert!(concealed_text(&view).iter().any(|c| c.contains("inside")));
        let kinds = view.overlay_kinds();
        assert_eq!(kinds.iter().filter(|k| k.starts_with("fold:")).count(), 3);
        // Fold the first heading: its section disappears.
        view.toggle_fold(0);
        assert!(concealed_text(&view).iter().any(|c| c.contains("one")));
        // Mode switches keep folds.
        view.set_mode(Mode::Reading);
        view.set_mode(Mode::Live);
        assert_eq!(view.folded().len(), 2);
        // The cursor going in unfolds.
        place(&view, text.find("one").unwrap());
        assert_eq!(view.folded(), [callout]);
        view.check_invariants().unwrap();
    }

    /// Block widgets span the text column without making the view's
    /// minimum width any larger, and still get clicks.
    #[gtk::test]
    async fn block_widgets_dont_set_a_minimum_width() {
        let text = "---\ntitle: Something long enough\n---\n# Heading\n";
        let view = view(text);
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&view)
            .build();
        let window = gtk::Window::builder()
            .default_width(900)
            .default_height(500)
            .child(&scrolled)
            .build();
        window.present();
        for _ in 0..20 {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let (min, _, _, _) = view.measure(gtk::Orientation::Horizontal, -1);
        assert!(min < 200, "the view needs {min}px");
        // The properties list spans most of the width and can be picked.
        let st = view.imp().state.borrow();
        let properties = st
            .overlays
            .iter()
            .find(|o| o.kind == OverlayKind::Properties)
            .and_then(|o| o.widget.borrow().clone())
            .unwrap();
        drop(st);
        assert!(
            properties.width() > 600,
            "only {}px wide",
            properties.width()
        );
        let bounds = properties.compute_bounds(&view).unwrap();
        let (x, y) = (bounds.x() + bounds.width() - 40.0, bounds.y() + 20.0);
        let picked = view
            .pick(f64::from(x), f64::from(y), gtk::PickFlags::DEFAULT)
            .unwrap();
        assert!(
            picked.is_ancestor(&properties) || picked == properties,
            "picked {}",
            picked.type_().name()
        );
        window.close();
    }

    #[gtk::test]
    fn display_math_is_rendered_until_the_cursor_enters() {
        let text = "Euler:\n\n$$\ne^{i\\pi} + 1 = 0\n$$\n\nafter\n";
        let view = view(text);
        place(&view, text.len());
        assert!(view.overlay_kinds().contains(&"math".to_owned()));
        assert!(concealed_text(&view).iter().any(|c| c.contains("e^{i")));
        place(&view, text.find("e^").unwrap());
        assert!(!view.overlay_kinds().contains(&"math".to_owned()));
        view.check_invariants().unwrap();
    }

    /// Obsidian's `$$` blocks may hold blank (or whitespace-only) lines, and
    /// work inside callouts.
    #[gtk::test]
    fn display_math_blocks_with_blank_lines_are_rendered() {
        let text = "Defined as:\n\n$$\n  \n\\mathrm{CIR}=1:1\n$$\n\n> [!note]\n> $$\n>\n> N \\geq 1\n> $$\n\nafter\n";
        let view = view(text);
        place(&view, text.len());
        let math = view.overlay_kinds().iter().filter(|k| *k == "math").count();
        assert_eq!(math, 2);
        let st = view.imp().state.borrow();
        for overlay in st.overlays.iter().filter(|o| o.kind == OverlayKind::Math) {
            let widget = view.make_widget(overlay, &st);
            assert!(widget.is::<gtk::Picture>(), "the formula didn't render");
        }
        drop(st);
        view.check_invariants().unwrap();
    }

    /// Typing above a widget keeps it: rebuilding every widget below the
    /// cursor on each keystroke cost 40 ms on a long note.
    #[gtk::test]
    fn widgets_survive_edits_above_them() {
        let text = "intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter\n";
        let view = view(text);
        place(&view, text.len());
        let table = || {
            let st = view.imp().state.borrow();
            let overlay = st
                .overlays
                .iter()
                .find(|o| o.kind == OverlayKind::Table)
                .unwrap();
            let widget = view.make_widget(overlay, &st);
            drop(widget);
            overlay.mark.clone()
        };
        let before = table();
        let buffer = view.buffer();
        buffer.insert(&mut buffer.start_iter(), "more ");
        assert_eq!(table(), before);
        view.check_invariants().unwrap();
    }

    /// The M0 budget: under 8 ms of work per keystroke on a 50 KB note.
    /// Run in release: `cargo test --release -p igneous-editor -- --ignored keystroke`.
    #[gtk::test]
    #[ignore = "timing"]
    async fn keystroke_cost_on_a_large_note() {
        let mut text = String::new();
        let mut i = 0;
        while text.len() < 50_000 {
            text.push_str(&format!(
                "## Section {i}\n\nSome **bold** and *italic* text with a [[Link {i}]] and #tag{i}.\n\n- [ ] task {i}\n- item with `code`\n\n> [!note] Callout {i}\n> body\n\n| a | b |\n|---|---|\n| {i} | x |\n\n"
            ));
            i += 1;
        }
        let view = view(&text);
        view.set_fold_headings(true);
        let window = gtk::Window::builder()
            .default_width(1000)
            .default_height(800)
            .child(&gtk::ScrolledWindow::builder().child(&view).build())
            .build();
        window.present();
        for _ in 0..10 {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        let buffer = view.buffer();
        let middle = buffer.char_count() / 2;
        let mut times = Vec::new();
        for n in 0..200 {
            let mut at = buffer.iter_at_offset(middle + n);
            let start = std::time::Instant::now();
            buffer.insert(&mut at, "x");
            times.push(start.elapsed());
        }
        times.sort();
        let p50 = times[times.len() / 2];
        let p95 = times[times.len() * 95 / 100];
        eprintln!("keystroke: p50 {p50:?}, p95 {p95:?} ({} bytes)", text.len());
        assert!(p95 < std::time::Duration::from_millis(8), "p95 {p95:?}");
        window.close();
    }

    #[test]
    fn table_rows() {
        assert_eq!(split_row("| a | b |"), [" a ", " b "]);
        assert_eq!(split_row("a|b"), ["a", "b"]);
        assert_eq!(split_row(r"| a \| b | `c|d` |"), [" a | b ", " `c|d` "]);
    }
}
