//! The note editor widget.
//!
//! M1 provides Source mode: GtkSourceView with Obsidian-flavoured Markdown
//! highlighting, Adwaita style schemes that follow the system, and a readable
//! line length. Live Preview (see docs/decisions/0001-editor.md) is added in M4.

mod buffer;
mod scheme;
pub mod theme;
mod view;

pub use buffer::{LANGUAGE_ID, new_buffer};
pub use scheme::{style_scheme, system_accent};
pub use view::NoteView;

use std::sync::Once;

/// Registers the editor's resources and language definitions. Call once after
/// GTK is initialised; later calls do nothing.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        gtk::gio::resources_register_include!("igneous-editor.gresource")
            .expect("editor resources are valid");
        sourceview::init();
        sourceview::LanguageManager::default()
            .append_search_path("resource:///dev/h4rl3y/igneous/editor/language-specs");
    });
}
