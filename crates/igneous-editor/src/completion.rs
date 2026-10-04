//! Completion while typing links and tags:
//!
//! - `[[` lists notes (and their aliases);
//! - `[[Note#` lists the note's headings, `[[Note#^` its blocks;
//! - `#` lists the vault's tags.
//!
//! What to offer comes from the editor's [`Host`](crate::Host).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

use gtk::{gio, glib, prelude::*, subclass::prelude::*};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use sourceview::subclass::prelude::*;

use crate::view::NoteView;

/// What's being completed, from the text before the cursor on its line.
/// `start` is where the typed query begins (a byte offset in the line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Note {
        query: String,
        start: usize,
    },
    Heading {
        target: String,
        query: String,
        start: usize,
    },
    Block {
        target: String,
        query: String,
        start: usize,
    },
    Tag {
        query: String,
        start: usize,
    },
}

impl Query {
    pub fn detect(line: &str) -> Option<Query> {
        if let Some(open) = line.rfind("[[")
            && !line[open..].contains("]]")
        {
            let inner = &line[open + 2..];
            if inner.contains('|') {
                return None;
            }
            let start = open + 2;
            return Some(match inner.split_once('#') {
                Some((target, sub)) => match sub.strip_prefix('^') {
                    Some(block) => Query::Block {
                        target: target.to_owned(),
                        query: block.to_owned(),
                        start: start + target.len() + 2,
                    },
                    None => Query::Heading {
                        target: target.to_owned(),
                        query: sub.to_owned(),
                        start: start + target.len() + 1,
                    },
                },
                None => Query::Note {
                    query: inner.to_owned(),
                    start,
                },
            });
        }
        // A tag: `#` at the start or after a space, then tag characters.
        let hash = line.rfind('#')?;
        let before_ok = line[..hash]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let query = &line[hash + 1..];
        let tag_chars = query
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'));
        // `# ` at a line's start is a heading, not a tag.
        (before_ok && tag_chars).then(|| Query::Tag {
            query: query.to_owned(),
            start: hash + 1,
        })
    }

    fn text(&self) -> &str {
        match self {
            Query::Note { query, .. }
            | Query::Heading { query, .. }
            | Query::Block { query, .. }
            | Query::Tag { query, .. } => query,
        }
    }

    fn start(&self) -> usize {
        match self {
            Query::Note { start, .. }
            | Query::Heading { start, .. }
            | Query::Block { start, .. }
            | Query::Tag { start, .. } => *start,
        }
    }
}

/// One suggestion: what's shown, and what's inserted.
#[derive(Debug, Clone, Default)]
pub struct Suggestion {
    pub label: String,
    pub detail: String,
    pub insert: String,
    pub icon: &'static str,
}

/// Suggestions matching `query`, best first.
pub fn rank(query: &str, candidates: Vec<Suggestion>, limit: usize) -> Vec<Suggestion> {
    let query = query.trim();
    if query.is_empty() {
        return candidates.into_iter().take(limit).collect();
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize, Suggestion)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let score = pattern.score(Utf32Str::new(&s.label, &mut buf), &mut matcher)?;
            Some((score, i, s))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, _, s)| s).collect()
}

// --- proposals -------------------------------------------------------------------

mod proposal_imp {
    use super::*;

    #[derive(Default)]
    pub struct Proposal {
        pub suggestion: RefCell<Suggestion>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Proposal {
        const NAME: &'static str = "IgneousCompletionProposal";
        type Type = super::Proposal;
        type Interfaces = (sourceview::CompletionProposal,);
    }

    impl ObjectImpl for Proposal {}
    impl CompletionProposalImpl for Proposal {}
}

glib::wrapper! {
    pub struct Proposal(ObjectSubclass<proposal_imp::Proposal>)
        @implements sourceview::CompletionProposal;
}

impl Proposal {
    fn new(suggestion: Suggestion) -> Self {
        let proposal: Self = glib::Object::new();
        proposal.imp().suggestion.replace(suggestion);
        proposal
    }

    fn suggestion(&self) -> Suggestion {
        self.imp().suggestion.borrow().clone()
    }
}

// --- the provider -------------------------------------------------------------------

const LIMIT: usize = 50;

mod provider_imp {
    use super::*;

    #[derive(Default)]
    pub struct LinkCompletion {
        pub view: glib::WeakRef<NoteView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LinkCompletion {
        const NAME: &'static str = "IgneousLinkCompletion";
        type Type = super::LinkCompletion;
        type Interfaces = (sourceview::CompletionProvider,);
    }

    impl ObjectImpl for LinkCompletion {}

    impl CompletionProviderImpl for LinkCompletion {
        fn title(&self) -> Option<glib::GString> {
            Some("Links and Tags".into())
        }

        fn priority(&self, _context: &sourceview::CompletionContext) -> i32 {
            // Above GtkSourceView's word completion.
            100
        }

        fn is_trigger(&self, iter: &gtk::TextIter, c: char) -> bool {
            let mut before = *iter;
            before.backward_chars(2);
            let previous = before.char();
            matches!(c, '#' | '^') || (c == '[' && previous == '[')
        }

        fn populate_future(
            &self,
            context: &sourceview::CompletionContext,
        ) -> Pin<Box<dyn Future<Output = Result<gio::ListModel, glib::Error>>>> {
            let model = self.obj().proposals(context);
            Box::pin(async move { Ok(model.upcast()) })
        }

        fn refilter(&self, context: &sourceview::CompletionContext, _model: &gio::ListModel) {
            let model = self.obj().proposals(context);
            context.set_proposals_for_provider(&*self.obj(), Some(&model));
        }

        fn display(
            &self,
            _context: &sourceview::CompletionContext,
            proposal: &sourceview::CompletionProposal,
            cell: &sourceview::CompletionCell,
        ) {
            let Some(proposal) = proposal.downcast_ref::<Proposal>() else {
                return;
            };
            let suggestion = proposal.suggestion();
            match cell.column() {
                sourceview::CompletionColumn::TypedText => cell.set_text(Some(&suggestion.label)),
                sourceview::CompletionColumn::Comment => cell.set_text(Some(&suggestion.detail)),
                sourceview::CompletionColumn::Icon => cell.set_icon_name(suggestion.icon),
                _ => cell.set_text(None),
            }
        }

        fn activate(
            &self,
            context: &sourceview::CompletionContext,
            proposal: &sourceview::CompletionProposal,
        ) {
            let (Some(proposal), Some(buffer)) =
                (proposal.downcast_ref::<Proposal>(), context.buffer())
            else {
                return;
            };
            accept(&buffer, &proposal.suggestion());
        }
    }
}

/// Puts `suggestion` in place of the query before the cursor.
pub(crate) fn accept(buffer: &sourceview::Buffer, suggestion: &Suggestion) {
    let Some((query, mut cursor)) = query_at_cursor(buffer) else {
        return;
    };
    let mut start = cursor;
    start.set_line_index(query.start() as i32);
    // Close a new link, unless it's closed already.
    let mut after = cursor;
    after.forward_chars(2);
    let closed = cursor.slice(&after) == "]]";
    let text = match query {
        Query::Note { .. } | Query::Heading { .. } | Query::Block { .. } if !closed => {
            format!("{}]]", suggestion.insert)
        }
        Query::Tag { .. } => format!("{} ", suggestion.insert),
        _ => suggestion.insert.clone(),
    };
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut cursor);
    buffer.insert(&mut start, &text);
    buffer.end_user_action();
}

glib::wrapper! {
    pub struct LinkCompletion(ObjectSubclass<provider_imp::LinkCompletion>)
        @implements sourceview::CompletionProvider;
}

/// The query before the cursor, and the cursor.
fn query_at_cursor(buffer: &sourceview::Buffer) -> Option<(Query, gtk::TextIter)> {
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let mut line_start = cursor;
    line_start.set_line_offset(0);
    let line = line_start.slice(&cursor);
    Some((Query::detect(&line)?, cursor))
}

impl LinkCompletion {
    pub fn new(view: &NoteView) -> Self {
        let provider: Self = glib::Object::new();
        provider.imp().view.set(Some(view));
        provider
    }

    fn proposals(&self, _context: &sourceview::CompletionContext) -> gio::ListStore {
        let store = gio::ListStore::new::<Proposal>();
        if let Some(view) = self.imp().view.upgrade() {
            for suggestion in suggestions(&view) {
                store.append(&Proposal::new(suggestion));
            }
        }
        store
    }
}

/// What to offer for the text before `view`'s cursor.
pub(crate) fn suggestions(view: &NoteView) -> Vec<Suggestion> {
    let buffer = view.source_buffer();
    let Some((query, _)) = query_at_cursor(&buffer) else {
        return Vec::new();
    };
    let host = view.host();
    let candidates: Vec<Suggestion> = match &query {
        Query::Note { .. } => host
            .note_names()
            .into_iter()
            .map(|n| Suggestion {
                insert: match &n.alias {
                    Some(alias) => format!("{}|{alias}", n.link),
                    None => n.link.clone(),
                },
                label: n.alias.clone().unwrap_or(n.title),
                detail: n.detail,
                icon: if n.alias.is_some() {
                    "mail-forward-symbolic"
                } else {
                    "text-x-generic-symbolic"
                },
            })
            .collect(),
        Query::Heading { target, .. } => {
            let headings = if target.is_empty() {
                igneous_markdown::parse(&view.model_text())
                    .headings
                    .into_iter()
                    .map(|h| h.text)
                    .collect()
            } else {
                host.headings(target)
            };
            headings
                .into_iter()
                .map(|h| Suggestion {
                    insert: h.clone(),
                    label: h,
                    detail: String::new(),
                    icon: "view-list-bullet-symbolic",
                })
                .collect()
        }
        Query::Block { target, .. } => {
            let blocks = if target.is_empty() {
                let text = view.model_text();
                let doc = igneous_markdown::parse(&text);
                doc.block_ids
                    .iter()
                    .map(|b| {
                        let start = igneous_markdown::text::line_start(&text, b.range.start);
                        (b.id.clone(), text[start..b.range.start].trim().to_owned())
                    })
                    .collect()
            } else {
                host.blocks(target)
            };
            blocks
                .into_iter()
                .map(|(id, text)| Suggestion {
                    insert: id.clone(),
                    label: id,
                    detail: text,
                    icon: "insert-link-symbolic",
                })
                .collect()
        }
        Query::Tag { .. } => host
            .tags()
            .into_iter()
            .map(|(tag, count)| Suggestion {
                insert: tag.clone(),
                label: tag,
                detail: if count == 1 {
                    "1 note".to_owned()
                } else {
                    format!("{count} notes")
                },
                icon: "tag-symbolic",
            })
            .collect(),
    };
    rank(query.text(), candidates, LIMIT)
}

impl NoteView {
    /// What completion would offer at the cursor. For tests.
    #[doc(hidden)]
    pub fn completion_labels(&self) -> Vec<String> {
        suggestions(self).into_iter().map(|s| s.label).collect()
    }

    /// Accepts the suggestion labelled `label`. For tests.
    #[doc(hidden)]
    pub fn complete_with(&self, label: &str) {
        if let Some(suggestion) = suggestions(self).into_iter().find(|s| s.label == label) {
            accept(&self.source_buffer(), &suggestion);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    struct Notes;
    impl crate::Host for Notes {
        fn note_names(&self) -> Vec<crate::NoteName> {
            ["Roadmap", "Home"]
                .into_iter()
                .map(|n| crate::NoteName {
                    link: n.into(),
                    title: n.into(),
                    detail: String::new(),
                    alias: None,
                })
                .collect()
        }
        fn headings(&self, target: &str) -> Vec<String> {
            if target == "Roadmap" {
                vec!["Plans".into(), "Done".into()]
            } else {
                vec![]
            }
        }
        fn tags(&self) -> Vec<(String, usize)> {
            vec![("project".into(), 3), ("personal".into(), 1)]
        }
    }

    fn typed(text: &str) -> NoteView {
        let view = NoteView::new();
        view.set_host(Rc::new(Notes));
        let buffer = view.source_buffer();
        buffer.set_text(text);
        buffer.place_cursor(&buffer.end_iter());
        view
    }

    #[gtk::test]
    fn completes_links_headings_and_tags() {
        let view = typed("See [[Road");
        assert_eq!(view.completion_labels(), ["Roadmap"]);
        view.complete_with("Roadmap");
        assert_eq!(view.model_text(), "See [[Roadmap]]");

        let view = typed("See [[Roadmap#Pl");
        assert_eq!(view.completion_labels(), ["Plans"]);
        view.complete_with("Plans");
        assert_eq!(view.model_text(), "See [[Roadmap#Plans]]");

        let view = typed("# Here\n[[#");
        assert_eq!(view.completion_labels(), ["Here"]);

        let view = typed("tagged #pro");
        assert_eq!(view.completion_labels()[0], "project");
        view.complete_with("project");
        assert_eq!(view.model_text(), "tagged #project ");
    }

    #[test]
    fn detects_what_is_typed() {
        assert_eq!(
            Query::detect("see [[Road"),
            Some(Query::Note {
                query: "Road".into(),
                start: 6
            })
        );
        assert_eq!(
            Query::detect("[[Roadmap#Pla"),
            Some(Query::Heading {
                target: "Roadmap".into(),
                query: "Pla".into(),
                start: 10
            })
        );
        assert_eq!(
            Query::detect("[[Roadmap#^ab"),
            Some(Query::Block {
                target: "Roadmap".into(),
                query: "ab".into(),
                start: 11
            })
        );
        assert_eq!(
            Query::detect("[[#Here"),
            Some(Query::Heading {
                target: "".into(),
                query: "Here".into(),
                start: 3
            })
        );
        assert_eq!(
            Query::detect("text #pro"),
            Some(Query::Tag {
                query: "pro".into(),
                start: 6
            })
        );
        assert_eq!(Query::detect("[[Done]] and more"), None);
        assert_eq!(Query::detect("[[Note|ali"), None);
        assert_eq!(Query::detect("# Heading"), None);
        assert_eq!(Query::detect("issue#12"), None);
    }

    #[test]
    fn ranks_by_label() {
        let s = |label: &str| Suggestion {
            label: label.into(),
            ..Suggestion::default()
        };
        let ranked = rank("road", vec![s("Home"), s("Roadmap"), s("Broad ideas")], 10);
        assert_eq!(ranked[0].label, "Roadmap");
        assert_eq!(ranked.len(), 2);
    }
}
