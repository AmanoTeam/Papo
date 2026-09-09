use gtk::{glib, prelude::*, subclass::prelude::*};
use relm4::gtk;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum TailDirection {
    #[default]
    Left,
    Right,
}

mod imp {
    use std::cell::Cell;

    use gtk::{glib, graphene, prelude::*, subclass::prelude::*};
    use relm4::gtk;

    use super::TailDirection;

    #[derive(Default)]
    pub struct MessageTail {
        pub(crate) direction: Cell<TailDirection>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageTail {
        const NAME: &'static str = "PapoMessageTail";
        type Type = super::MessageTail;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MessageTail {}

    impl WidgetImpl for MessageTail {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let width = u16::try_from(widget.width()).unwrap_or(0);
            let height = u16::try_from(widget.height()).unwrap_or(0);
            if width == 0 || height == 0 {
                return;
            }

            let color = widget.color();
            let bounds = graphene::Rect::new(0.0, 0.0, f32::from(width), f32::from(height));
            let cr = snapshot.append_cairo(&bounds);
            cr.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()),
            );

            let w = f64::from(width);
            let h = f64::from(height);

            match self.direction.get() {
                TailDirection::Left => {
                    cr.move_to(w, 0.0);
                    cr.line_to(w, h);
                    cr.line_to(0.0, h);
                    cr.curve_to(w * 0.6, h * 0.95, w, h * 0.3, w, 0.0);
                }
                TailDirection::Right => {
                    cr.move_to(0.0, 0.0);
                    cr.line_to(0.0, h);
                    cr.line_to(w, h);
                    cr.curve_to(w * 0.4, h * 0.95, 0.0, h * 0.3, 0.0, 0.0);
                }
            }
            cr.close_path();
            let _ = cr.fill();
        }
    }
}

glib::wrapper! {
    pub struct MessageTail(ObjectSubclass<imp::MessageTail>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for MessageTail {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageTail {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_direction(&self, direction: TailDirection) {
        let imp = self.imp();
        if imp.direction.get() != direction {
            imp.direction.set(direction);
            self.queue_draw();
        }
    }
}
