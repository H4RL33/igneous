//! What a note's editor asks of the window: which links exist, where images
//! are, what embeds contain, and following links.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::{glib, prelude::*};
use igneous_core::VaultPath;
use igneous_core::fs::read_text;
use igneous_editor::{Embed, Host};
use igneous_markdown::{LinkRef, Subpath, parse, subpath_range};

use crate::note_page::NotePage;
use crate::vault::VaultContext;
use crate::window::Window;

pub struct NoteHost {
    pub ctx: Rc<VaultContext>,
    pub page: glib::WeakRef<NotePage>,
}

impl NoteHost {
    fn from(&self) -> Option<VaultPath> {
        self.page.upgrade().and_then(|p| p.path())
    }

    fn window(&self) -> Option<Window> {
        self.page.upgrade()?.root().and_downcast::<Window>()
    }
}

impl Host for NoteHost {
    fn link_exists(&self, link: &LinkRef) -> bool {
        self.ctx.resolve(link, self.from().as_ref()).is_some()
    }

    fn image_path(&self, target: &str) -> Option<PathBuf> {
        let link = LinkRef {
            target: target.to_owned(),
            ..LinkRef::default()
        };
        self.ctx
            .resolve(&link, self.from().as_ref())
            .map(|p| self.ctx.abs(&p))
    }

    fn embed(&self, link: &LinkRef) -> Option<Embed> {
        let path = self.ctx.resolve(link, self.from().as_ref())?;
        let (file, _) = read_text(&self.ctx.abs(&path)).ok()?;
        let text = file.text();
        let mut title = crate::files::display_name(&path, false).0;
        let body = match &link.subpath {
            None => strip_frontmatter(text).to_owned(),
            Some(subpath) => {
                let doc = parse(text);
                let range = subpath_range(&doc, text, subpath)?;
                if let Subpath::Heading(path) = subpath {
                    title = format!("{title} › {}", path.join(" › "));
                }
                text[range].to_owned()
            }
        };
        Some(Embed { title, text: body })
    }

    fn open_link(&self, link: &LinkRef, new_tab: bool) {
        if let Some(window) = self.window() {
            window.follow_link(link, self.from().as_ref(), new_tab);
        }
    }
}

fn strip_frontmatter(text: &str) -> &str {
    match igneous_markdown::frontmatter::detect(text) {
        Some((range, _)) => &text[range.end..],
        None => text,
    }
}
