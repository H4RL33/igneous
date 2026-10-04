//! The note editor widget: Live Preview, Source and Reading modes on
//! GtkSourceView (see docs/decisions/0001-editor.md), coloured by editor
//! themes.

mod buffer;
pub mod completion;
mod host;
mod hover;
mod live;
pub mod markup;
mod rangeset;
mod scheme;
mod tags;
pub mod theme;
mod view;

pub use buffer::LANGUAGE_ID;
pub use host::{Embed, Host, NoHost, NoteName};
pub use scheme::{style_scheme, system_accent};
pub use view::{Mode, NoteView};

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
