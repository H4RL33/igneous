//! A tab showing an image from the vault.

use std::cell::RefCell;
use std::path::Path;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib, graphene, gsk};
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
        // GtkPicture loads the file (whatever formats GTK knows), and the
        // image is drawn with rounded corners like the editor's blocks.
        let paintable = gtk::Picture::for_filename(abs).paintable();
        let picture = RoundedPicture::new(paintable.as_ref());
        picture.update_property(&[gtk::accessible::Property::Label(path.file_name())]);
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

/// The corner radius of images, the same as the editor's blocks.
const RADIUS: f32 = 8.0;

mod rounded_imp {
    use super::*;

    #[derive(Default)]
    pub struct RoundedPicture {
        pub paintable: RefCell<Option<gdk::Paintable>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoundedPicture {
        const NAME: &'static str = "IgneousRoundedPicture";
        type Type = super::RoundedPicture;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for RoundedPicture {}

    impl WidgetImpl for RoundedPicture {
        /// Shrinks to fit, never grows past the image's own size.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let natural = self.paintable.borrow().as_ref().map_or(0, |p| {
                if orientation == gtk::Orientation::Horizontal {
                    p.intrinsic_width()
                } else {
                    p.intrinsic_height()
                }
            });
            (0, natural.max(0), -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(paintable) = self.paintable.borrow().clone() else {
                return;
            };
            let widget = self.obj();
            let (width, height) = (f64::from(widget.width()), f64::from(widget.height()));
            let (image_width, image_height) = (
                f64::from(paintable.intrinsic_width()),
                f64::from(paintable.intrinsic_height()),
            );
            let (w, h) = if image_width > 0.0 && image_height > 0.0 {
                let scale = (width / image_width).min(height / image_height).min(1.0);
                (image_width * scale, image_height * scale)
            } else {
                (width, height)
            };
            let (x, y) = ((width - w) / 2.0, (height - h) / 2.0);
            let rect = graphene::Rect::new(x as f32, y as f32, w as f32, h as f32);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, RADIUS));
            snapshot.save();
            snapshot.translate(&graphene::Point::new(x as f32, y as f32));
            paintable.snapshot(snapshot, w, h);
            snapshot.restore();
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    /// An image scaled down to fit, centred, with rounded corners: the
    /// corners are the image's own, not those of a frame around it.
    pub struct RoundedPicture(ObjectSubclass<rounded_imp::RoundedPicture>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl RoundedPicture {
    pub fn new(paintable: Option<&gdk::Paintable>) -> Self {
        let picture: Self = glib::Object::new();
        if let Some(paintable) = paintable {
            // Animations and images that load late redraw themselves.
            paintable.connect_invalidate_contents(glib::clone!(
                #[weak]
                picture,
                move |_| picture.queue_draw()
            ));
            paintable.connect_invalidate_size(glib::clone!(
                #[weak]
                picture,
                move |_| picture.queue_resize()
            ));
        }
        picture.imp().paintable.replace(paintable.cloned());
        picture
    }
}
