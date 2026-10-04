//! Previews of linked notes: hold Ctrl and rest the pointer on a link.

use std::future::Future;
use std::pin::Pin;

use gtk::{gdk, glib, prelude::*, subclass::prelude::*};
use sourceview::subclass::prelude::*;

use crate::view::NoteView;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LinkPreview {
        pub view: glib::WeakRef<NoteView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LinkPreview {
        const NAME: &'static str = "IgneousLinkPreview";
        type Type = super::LinkPreview;
        type Interfaces = (sourceview::HoverProvider,);
    }

    impl ObjectImpl for LinkPreview {}

    impl HoverProviderImpl for LinkPreview {
        fn populate_future(
            &self,
            context: &sourceview::HoverContext,
            display: &sourceview::HoverDisplay,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let shown = self
                .view
                .upgrade()
                .filter(|_| ctrl_held())
                .zip(context.iter())
                .and_then(|(view, iter)| view.preview_at(&iter));
            let result = match shown {
                Some(widget) => {
                    display.append(&widget);
                    Ok(())
                }
                None => Err(glib::Error::new(
                    gtk::gio::IOErrorEnum::NotFound,
                    "no link here",
                )),
            };
            Box::pin(async move { result })
        }
    }
}

glib::wrapper! {
    pub struct LinkPreview(ObjectSubclass<imp::LinkPreview>)
        @implements sourceview::HoverProvider;
}

impl LinkPreview {
    pub fn new(view: &NoteView) -> Self {
        let preview: Self = glib::Object::new();
        preview.imp().view.set(Some(view));
        preview
    }
}

fn ctrl_held() -> bool {
    gdk::Display::default()
        .and_then(|d| d.default_seat())
        .and_then(|s| s.keyboard())
        .is_some_and(|k| k.modifier_state().contains(gdk::ModifierType::CONTROL_MASK))
}
