use adw::prelude::*;
use relm4::prelude::*;

#[relm4::widget_template(pub)]
impl WidgetTemplate for PairingCell {
    type Init = char;

    view! {
        gtk::Box {
            inline_css: "padding-top: 2px; padding-left: 6px; padding-right: 6px; padding-bottom: 2px;",
            set_halign: gtk::Align::Center,
            set_valign: gtk::Align::Center,
            set_hexpand: true,
            set_vexpand: true,
            set_css_classes: &["card", "frame"],

            #[name = "character"]
            gtk::Label {
                set_label: init.to_string().as_str(),
                set_halign: gtk::Align::Center,
                set_valign: gtk::Align::Center,
                set_justify: gtk::Justification::Center,
                set_css_classes: &["title-3", "accent"]
            }
        }
    }
}
