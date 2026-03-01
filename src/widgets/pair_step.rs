use adw::prelude::*;
use relm4::prelude::*;

#[relm4::widget_template(pub)]
impl WidgetTemplate for PairStep {
    view! {
        gtk::Box {
            set_halign: gtk::Align::Start,
            set_hexpand: true,
            set_spacing: 10,
            set_orientation: gtk::Orientation::Horizontal,

            gtk::Box {
                inline_css: r"
                    border-radius: 50%;
                    background-color: var(--accent-bg-color);
                ",
                set_width_request: 30,
                set_height_request: 30,

                #[name = "number"]
                gtk::Label {
                    set_halign: gtk::Align::Center,
                    set_hexpand: true,
                    set_justify: gtk::Justification::Center,
                    set_css_classes: &["title-4"]
                }
            },

            #[name = "step"]
            gtk::Label {
                set_hexpand: true,
                set_justify: gtk::Justification::Left,
                set_use_markup: true,
                set_css_classes: &["body"]
            }
        }
    }
}
