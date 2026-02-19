use adw::prelude::*;
use relm4::prelude::*;

use crate::{
    config::{APP_ID, VERSION},
    i18n,
};

pub struct AboutDialog;

impl SimpleComponent for AboutDialog {
    type Init = ();
    type Root = adw::AboutDialog;
    type Input = ();
    type Output = ();
    type Widgets = adw::AboutDialog;

    fn init_root() -> Self::Root {
        adw::AboutDialog::builder()
            .application_name("Papo")
            .application_icon(APP_ID)
            .license_type(gtk::License::Apache20)
            .website("https://github.com/AmanoTeam/Papo")
            .issue_url("https://github.com/AmanoTeam/Papo/issues")
            .version(VERSION)
            .copyright("© 2026 Andriel Ferreira")
            .developers(["Andriel Ferreira"])
            .designers(["Andriel Ferreira"])
            .translator_credits(i18n!("translator-credits"))
            .build()
    }

    fn init(
        _: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Self {};

        let widgets = root.clone();
        widgets.present(Some(&relm4::main_adw_application().windows()[0]));

        ComponentParts { model, widgets }
    }

    fn update_view(&self, _dialog: &mut Self::Widgets, _sender: ComponentSender<Self>) {}
}
