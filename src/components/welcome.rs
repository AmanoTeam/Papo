use adw::prelude::*;
use relm4::prelude::*;

use crate::i18n;

#[derive(Debug)]
pub struct Welcome;

#[derive(Debug)]
pub enum WelcomeOutput {
    /// User chose to pair with QR code.
    PairWithQrCode,
    /// User chose to pair with phone number.
    PairWithPhoneNumber,
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for Welcome {
    type Init = ();
    type Input = ();
    type Output = WelcomeOutput;

    view! {
        &adw::StatusPage {
            set_title: &i18n!("Welcome to Papo"),
            set_description: Some(&i18n!("Connect to WhatsApp")),
            set_vexpand: true,
            set_icon_name: Some("chat-bubbles-text-symbolic"),
            set_css_classes: &["welcome-page"],

            gtk::Box {
                set_halign: gtk::Align::Center,
                set_spacing: 12,
                set_orientation: gtk::Orientation::Vertical,

                gtk::Button {
                    set_label: &i18n!("Link with QR Code"),
                    set_css_classes: &["pill", "suggested-action", "large"],
                    set_width_request: 250,

                    connect_clicked[sender] => move |_| {
                        let _ = sender.output(WelcomeOutput::PairWithQrCode);
                    }
                },

                gtk::Button {
                    set_label: &i18n!("Link with Phone Number"),
                    set_css_classes: &["pill", "large"],
                    set_width_request: 250,

                    connect_clicked[sender] => move |_| {
                        let _ = sender.output(WelcomeOutput::PairWithPhoneNumber);
                    }
                }
            }
        }
    }

    async fn init(
        _init: Self::Init,
        _root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let model = Self;
        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, _input: Self::Input, _sender: AsyncComponentSender<Self>) {}
}
