//! A tab showing an image from the vault.

use std::cell::RefCell;
use std::path::Path;

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;
use igneous_core::VaultPath;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ImagePage {
        pub path: RefCell<Option<VaultPath>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImagePage {
        const NAME: &'static str = "IgneousImagePage";
        type Type = super::ImagePage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ImagePage {}
    impl WidgetImpl for ImagePage {}
    impl BinImpl for ImagePage {}
}

glib::wrapper! {
    pub struct ImagePage(ObjectSubclass<imp::ImagePage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ImagePage {
    pub fn new(path: &VaultPath, abs: &Path) -> Self {
        let page: Self = glib::Object::new();
        let picture = gtk::Picture::for_filename(abs);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::ScaleDown);
        picture.set_alternative_text(Some(path.file_name()));
        picture.set_margin_top(12);
        picture.set_margin_bottom(12);
        picture.set_margin_start(12);
        picture.set_margin_end(12);
        page.set_child(Some(&picture));
        page.imp().path.replace(Some(path.clone()));
        page
    }

    pub fn path(&self) -> Option<VaultPath> {
        self.imp().path.borrow().clone()
    }

    pub fn set_path(&self, path: VaultPath) {
        self.imp().path.replace(Some(path));
    }
}

/// Image formats GTK can show.
pub fn is_image(path: &VaultPath) -> bool {
    matches!(
        path.extension().map(str::to_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "avif" | "tif" | "tiff")
    )
}
