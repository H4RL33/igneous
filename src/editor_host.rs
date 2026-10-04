//! What a note's editor asks of the window: which links exist, where images
//! are, what embeds contain, and following links.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{glib, prelude::*};
use igneous_core::VaultPath;
use igneous_core::fs::read_text;
use igneous_core::settings::{self as vault_settings, PropertyType, VaultSettings};
use igneous_editor::{Embed, Host, NoteName, PropertyKind};
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

    fn note_names(&self) -> Vec<NoteName> {
        let Some(window) = self.window() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for note in window.index().notes().iter() {
            let link = self.ctx.link_text(&note.path);
            let detail = note
                .path
                .parent()
                .map(|p| p.to_string())
                .unwrap_or_default();
            for alias in &note.aliases {
                names.push(NoteName {
                    link: link.clone(),
                    title: note.title.clone(),
                    detail: format!("Alias of {}", note.title),
                    alias: Some(alias.clone()),
                });
            }
            names.push(NoteName {
                link,
                title: note.title.clone(),
                detail,
                alias: None,
            });
        }
        names
    }

    fn headings(&self, target: &str) -> Vec<String> {
        self.parsed(target)
            .map(|(_, doc)| doc.headings.into_iter().map(|h| h.text).collect())
            .unwrap_or_default()
    }

    fn blocks(&self, target: &str) -> Vec<(String, String)> {
        let Some((text, doc)) = self.parsed(target) else {
            return Vec::new();
        };
        doc.block_ids
            .iter()
            .map(|b| {
                let start = igneous_markdown::text::line_start(&text, b.range.start);
                (b.id.clone(), text[start..b.range.start].trim().to_owned())
            })
            .collect()
    }

    fn fix(&self, rule: &str) {
        if let (Some(window), Some(page)) = (self.window(), self.page.upgrade()) {
            window.fix_rule(&page, rule);
        }
    }

    fn tags(&self) -> Vec<(String, usize)> {
        self.window()
            .map(|w| w.index().tags().as_ref().clone())
            .unwrap_or_default()
    }

    fn property_kind(&self, key: &str) -> Option<PropertyKind> {
        let window = self.window()?;
        let properties = window.index().properties();
        let info = properties.iter().find(|p| p.key == key)?;
        Some(kind_of(info.ty))
    }

    fn set_property_kind(&self, key: &str, kind: PropertyKind) {
        let Some(window) = self.window() else { return };
        let dir = self.ctx.vault.igneous_dir();
        // Never overwrite a vault.json that can't be read.
        let mut settings = match vault_settings::load::<VaultSettings>(&dir) {
            Ok(settings) => settings,
            Err(e) => {
                window.toast(&format!("Couldn’t save the property type: {e}"));
                return;
            }
        };
        settings
            .properties
            .types
            .insert(key.to_owned(), type_of(kind));
        if let Err(e) = vault_settings::save(&dir, &settings) {
            window.toast(&format!("Couldn’t save the property type: {e}"));
            return;
        }
        window.index().set_overrides(settings.properties.types);
    }

    fn save_attachment(&self, name: &str, bytes: &[u8]) -> Option<String> {
        // Obsidian names pasted images "Pasted image 20261004093000.png".
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let stamp = glib::DateTime::now_local()
            .ok()?
            .format("%Y%m%d%H%M%S")
            .ok()?;
        let path = self.attachment_path(&format!("{stem} {stamp}"), ext)?;
        self.write_attachment(&path, bytes)
    }

    fn attach_file(&self, source: &Path) -> Option<String> {
        // Already in the vault: just link to it.
        if let Some(path) = self.ctx.vault.relative(source)
            && !self.ctx.vault.is_ignored(&path)
        {
            return Some(format!("![[{}]]", self.ctx.link_text(&path)));
        }
        let name = source.file_name()?.to_string_lossy().into_owned();
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name.as_str(), ""));
        let path = self.attachment_path(stem, ext)?;
        let bytes = std::fs::read(source).ok()?;
        self.write_attachment(&path, &bytes)
    }

    fn property_keys(&self) -> Vec<String> {
        self.window()
            .map(|w| {
                w.index()
                    .properties()
                    .iter()
                    .map(|p| p.key.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn kind_of(ty: PropertyType) -> PropertyKind {
    match ty {
        PropertyType::Text => PropertyKind::Text,
        PropertyType::List => PropertyKind::List,
        PropertyType::Number => PropertyKind::Number,
        PropertyType::Checkbox => PropertyKind::Checkbox,
        PropertyType::Date => PropertyKind::Date,
        PropertyType::Datetime => PropertyKind::DateTime,
        PropertyType::Tags => PropertyKind::Tags,
        PropertyType::Aliases => PropertyKind::Aliases,
    }
}

fn type_of(kind: PropertyKind) -> PropertyType {
    match kind {
        PropertyKind::Text => PropertyType::Text,
        PropertyKind::List => PropertyType::List,
        PropertyKind::Number => PropertyType::Number,
        PropertyKind::Checkbox => PropertyType::Checkbox,
        PropertyKind::Date => PropertyType::Date,
        PropertyKind::DateTime => PropertyType::Datetime,
        PropertyKind::Tags => PropertyType::Tags,
        PropertyKind::Aliases => PropertyType::Aliases,
    }
}

impl NoteHost {
    /// A free path for a new attachment, in the vault's attachment folder.
    fn attachment_path(&self, stem: &str, ext: &str) -> Option<VaultPath> {
        use igneous_core::settings::Location;
        let note_folder = self.from().and_then(|p| p.parent());
        let folder = match &self.ctx.settings.files.attachment_location {
            Location::VaultRoot => None,
            Location::SameFolder => note_folder,
            Location::Folder(f) => VaultPath::new(f).ok(),
            Location::Subfolder(sub) => match note_folder {
                Some(base) => base.join(sub).ok(),
                None => VaultPath::new(sub).ok(),
            },
        };
        if let Some(folder) = &folder {
            std::fs::create_dir_all(self.ctx.abs(folder)).ok()?;
        }
        Some(self.ctx.vault.unused_name(folder.as_ref(), stem, ext))
    }

    fn write_attachment(&self, path: &VaultPath, bytes: &[u8]) -> Option<String> {
        let window = self.window()?;
        if let Err(e) = igneous_core::fs::write_bytes_atomic(
            &self.ctx.abs(path),
            bytes,
            igneous_core::fs::Expect::Absent,
        ) {
            window.toast(&format!("Couldn’t save the attachment: {e}"));
            return None;
        }
        window.on_vault_events(vec![igneous_core::watch::VaultEvent::Created(path.clone())]);
        Some(format!("![[{}]]", self.ctx.link_text(path)))
    }

    /// The text and parse of the note `target` names.
    fn parsed(&self, target: &str) -> Option<(String, igneous_markdown::Document)> {
        let link = LinkRef {
            target: target.to_owned(),
            ..LinkRef::default()
        };
        let path = self.ctx.resolve(&link, self.from().as_ref())?;
        let (file, _) = read_text(&self.ctx.abs(&path)).ok()?;
        let text = file.text().to_owned();
        let doc = parse(&text);
        Some((text, doc))
    }
}

fn strip_frontmatter(text: &str) -> &str {
    match igneous_markdown::frontmatter::detect(text) {
        Some((range, _)) => &text[range.end..],
        None => text,
    }
}
