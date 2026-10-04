//! Problems found in a note (by the linter), underlined in the text, with an
//! explanation and a Fix button when the pointer rests on one.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;

use gtk::{glib, prelude::*, subclass::prelude::*};
use sourceview::subclass::prelude::*;

use crate::view::{Mode, NoteView};

/// The tag key Live Preview uses for the underline.
pub(crate) const KEY: &str = "diagnostic";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte range in the note. An empty range is widened to a character.
    pub range: Range<usize>,
    /// A short summary, such as the rule's name.
    pub message: String,
    /// A longer explanation, shown below the summary.
    pub explanation: String,
    /// The rule that found it, passed to [`Host::fix`](crate::Host::fix).
    pub rule: String,
}

/// Moves diagnostics past an edit, dropping any the edit touched.
pub(crate) fn shift(
    diagnostics: &mut Vec<Diagnostic>,
    pos: usize,
    deleted: usize,
    inserted: usize,
) {
    let end = pos + deleted;
    diagnostics.retain_mut(|d| {
        if d.range.end <= pos && !(d.range.is_empty() && d.range.start == pos) {
            true
        } else if d.range.start >= end && (deleted > 0 || d.range.start > pos) {
            d.range = d.range.start - deleted + inserted..d.range.end - deleted + inserted;
            true
        } else {
            false
        }
    });
}

/// `range` within `text`, on character boundaries, at least one character
/// long where the text allows.
fn widen(text: &str, range: &Range<usize>) -> Option<Range<usize>> {
    let mut start = range.start.min(text.len());
    let mut end = range.end.clamp(start, text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end += 1;
    }
    if start == end {
        // Underline the character after (or, at the end, before) the spot.
        if let Some(c) = text[start..].chars().next().filter(|c| *c != '\n') {
            end = start + c.len_utf8();
        } else {
            start -= text[..start].chars().next_back()?.len_utf8();
        }
    }
    Some(start..end)
}

impl NoteView {
    /// Underlines `diagnostics` (byte ranges in the current text), replacing
    /// any shown before. Edits move them along; ones an edit touches go.
    pub fn set_diagnostics(&self, diagnostics: Vec<Diagnostic>) {
        {
            let mut st = self.imp().state.borrow_mut();
            let text = st.text.clone();
            st.diagnostics = diagnostics
                .into_iter()
                .filter_map(|mut d| {
                    d.range = widen(&text, &d.range)?;
                    Some(d)
                })
                .collect();
            let mut desired = std::collections::HashMap::new();
            desired.insert(KEY.to_owned(), coverage(&st.diagnostics, st.mode));
            self.apply(&mut st, desired, &[KEY.to_owned()]);
        }
        self.queue_draw();
    }

    /// The problems shown now. For tests.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.imp().state.borrow().diagnostics.clone()
    }

    fn diagnostic_at(&self, iter: &gtk::TextIter) -> Option<Diagnostic> {
        let st = self.imp().state.try_borrow().ok()?;
        let line_start = *st.lines.get(iter.line() as usize)?;
        let pos = line_start + iter.line_index() as usize;
        st.diagnostics
            .iter()
            .find(|d| d.range.start <= pos && pos < d.range.end)
            .cloned()
    }
}

/// What the underline covers in `mode` (nothing while reading).
pub(crate) fn coverage(diagnostics: &[Diagnostic], mode: Mode) -> Vec<Range<usize>> {
    if mode == Mode::Reading {
        return Vec::new();
    }
    diagnostics.iter().map(|d| d.range.clone()).collect()
}

// --- the hover --------------------------------------------------------------

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ProblemHover {
        pub view: glib::WeakRef<NoteView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProblemHover {
        const NAME: &'static str = "IgneousProblemHover";
        type Type = super::ProblemHover;
        type Interfaces = (sourceview::HoverProvider,);
    }

    impl ObjectImpl for ProblemHover {}

    impl HoverProviderImpl for ProblemHover {
        fn populate_future(
            &self,
            context: &sourceview::HoverContext,
            display: &sourceview::HoverDisplay,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let found = self
                .view
                .upgrade()
                .filter(|v| v.mode() != Mode::Reading)
                .zip(context.iter())
                .and_then(|(view, iter)| Some((view.diagnostic_at(&iter)?, view)));
            let result = match found {
                Some((diagnostic, view)) => {
                    display.append(&problem_widget(&view, &diagnostic));
                    Ok(())
                }
                None => Err(glib::Error::new(
                    gtk::gio::IOErrorEnum::NotFound,
                    "no problem here",
                )),
            };
            Box::pin(async move { result })
        }
    }
}

glib::wrapper! {
    pub struct ProblemHover(ObjectSubclass<imp::ProblemHover>)
        @implements sourceview::HoverProvider;
}

impl ProblemHover {
    pub fn new(view: &NoteView) -> Self {
        let hover: Self = glib::Object::new();
        hover.imp().view.set(Some(view));
        hover
    }
}

fn problem_widget(view: &NoteView, diagnostic: &Diagnostic) -> gtk::Widget {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(6)
        .margin_end(6)
        .margin_top(4)
        .margin_bottom(4)
        .build();
    content.append(
        &gtk::Label::builder()
            .label(&diagnostic.message)
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(48)
            .css_classes(["heading"])
            .build(),
    );
    if !diagnostic.explanation.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(&diagnostic.explanation)
                .xalign(0.0)
                .wrap(true)
                .max_width_chars(48)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    if diagnostic.rule != "yaml" {
        let fix = gtk::Button::builder()
            .label("_Fix")
            .use_underline(true)
            .halign(gtk::Align::Start)
            .margin_top(4)
            .tooltip_text("Apply this rule to the note")
            .css_classes(["pill"])
            .build();
        let rule = diagnostic.rule.clone();
        let weak = view.downgrade();
        fix.connect_clicked(move |button| {
            if let Some(view) = weak.upgrade() {
                view.host().fix(&rule);
            }
            // Close the popover now the problem is gone.
            if let Some(popover) = button.ancestor(gtk::Popover::static_type()) {
                popover.downcast::<gtk::Popover>().unwrap().popdown();
            }
        });
        content.append(&fix);
    }
    content.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(range: Range<usize>) -> Diagnostic {
        Diagnostic {
            range,
            message: String::new(),
            explanation: String::new(),
            rule: "r".into(),
        }
    }

    #[test]
    fn edits_move_or_drop_diagnostics() {
        let mut list = vec![d(0..2), d(5..7), d(10..12)];
        shift(&mut list, 6, 1, 0); // inside the second
        assert_eq!(
            list.iter().map(|d| d.range.clone()).collect::<Vec<_>>(),
            [0..2, 9..11]
        );
        shift(&mut list, 3, 0, 4); // between
        assert_eq!(
            list.iter().map(|d| d.range.clone()).collect::<Vec<_>>(),
            [0..2, 13..15]
        );
    }

    #[test]
    fn widening() {
        assert_eq!(widen("ab\n", &(1..1)), Some(1..2));
        assert_eq!(widen("ab\n", &(2..2)), Some(1..2));
        assert_eq!(widen("aé", &(2..2)), Some(1..3));
        assert_eq!(widen("", &(0..0)), None);
        assert_eq!(widen("abc", &(1..9)), Some(1..3));
    }
}
