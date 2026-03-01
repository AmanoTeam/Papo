use adw::prelude::*;
use relm4::prelude::*;

#[relm4::widget_template(pub)]
impl WidgetTemplate for PairingCell {
    type Init = char;

    view! {
        gtk::Box {
            set_halign: gtk::Align::Center,
            set_valign: gtk::Align::Center,
            set_hexpand: true,
            set_vexpand: true,
            set_css_classes: &["card", "frame"],
            set_width_request: 35,
            set_height_request: 35,

            #[name = "character"]
            gtk::Label {
                set_label: init.to_string().as_str(),
                set_halign: gtk::Align::Center,
                set_valign: gtk::Align::Center,
                set_hexpand: true,
                set_vexpand: true,
                set_justify: gtk::Justification::Center,
                set_css_classes: &["title-3", "accent"]
            }
        }
    }
}
