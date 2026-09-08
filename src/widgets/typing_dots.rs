use gtk::glib;
use relm4::gtk;

mod imp {
    use std::{cell::Cell, time::Duration};

    use gtk::{glib, graphene, prelude::*, subclass::prelude::*};
    use relm4::gtk;

    const CYCLE: f64 = 6.0;
    const DESCENT: f64 = 3.0;
    const UNIT_MS: f64 = 150.0;
    const RISE_START: f64 = 4.5;

    fn offset_at(phase: f64) -> f64 {
        let p = phase.rem_euclid(CYCLE);
        if p < DESCENT {
            0.5 + 0.5 * (std::f64::consts::PI * p / DESCENT).cos()
        } else if p > RISE_START {
            0.5 - 0.5 * (std::f64::consts::PI * (p - RISE_START) / (CYCLE - RISE_START)).cos()
        } else {
            0.0
        }
    }

    #[derive(Default)]
    pub struct TypingDots {
        pub(crate) phase: Cell<f64>,
        pub(crate) start_us: Cell<Option<i64>>,
        pub(crate) tick_source: Cell<Option<gtk::TickCallbackId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypingDots {
        const NAME: &'static str = "PapoTypingDots";
        type Type = super::TypingDots;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TypingDots {}

    impl WidgetImpl for TypingDots {
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
            let radius = (h * 0.16).max(1.5);
            let raise = h * 0.30;
            let step = (radius * 3.5).min((w - radius * 2.0) / 2.0).max(0.0);
            let x0 = (w - step * 2.0) / 2.0;
            let baseline = h - radius - 1.0;

            let phase = self.phase.get();

            for (x, lag) in [(x0, 0.0), (x0 + step, 1.0), (x0 + step * 2.0, 2.0)] {
                let offset = offset_at(phase - lag);
                let y = baseline - raise * offset;
                cr.move_to(x + radius, y);
                cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
            }
            let _ = cr.fill();
        }

        fn map(&self) {
            self.parent_map();

            let source = self.obj().add_tick_callback(|widget, clock| {
                let imp = widget.imp();
                let now = clock.frame_time();
                let start = imp.start_us.get().unwrap_or_else(|| {
                    imp.start_us.set(Some(now));
                    now
                });
                let elapsed = Duration::from_micros(u64::try_from(now - start).unwrap_or_default());
                imp.phase.set(elapsed.as_secs_f64() * 1000.0 / UNIT_MS);

                widget.queue_draw();
                glib::ControlFlow::Continue
            });
            self.tick_source.set(Some(source));
        }

        fn unmap(&self) {
            if let Some(source) = self.tick_source.take() {
                source.remove();
            }
            self.start_us.set(None);

            self.parent_unmap();
        }
    }
}

glib::wrapper! {
    pub struct TypingDots(ObjectSubclass<imp::TypingDots>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for TypingDots {
    fn default() -> Self {
        Self::new()
    }
}

impl TypingDots {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("width-request", 24)
            .property("height-request", 14)
            .build()
    }
}
