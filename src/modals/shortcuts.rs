use adw::prelude::*;
use relm4::prelude::*;

use crate::i18n;

pub struct ShortcutsDialog;

impl SimpleComponent for ShortcutsDialog {
    type Init = ();
    type Root = adw::ShortcutsDialog;
    type Input = ();
    type Output = ();
    type Widgets = adw::ShortcutsDialog;

    fn init_root() -> Self::Root {
        adw::ShortcutsDialog::builder().build()
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Self {};
        let widgets = root.clone();

        // Shortcuts section
        let section = adw::ShortcutsSection::new(Some(&i18n!("General")));
        section.add(adw::ShortcutsItem::new(&i18n!("Quit"), "<Control>q"));
        // section.add(adw::ShortcutsItem::new("New Tab", "<Control>t"));

        widgets.add(section);
        widgets.present(Some(&relm4::main_adw_application().windows()[0]));

        ComponentParts { model, widgets }
    }
}
