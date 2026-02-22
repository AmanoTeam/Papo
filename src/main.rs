#![deny(clippy::all)]
#![deny(clippy::cargo)]
#![deny(clippy::pedantic)]
// To keep consistency.
#![deny(clippy::use_self)]
#![deny(clippy::redundant_clone)]
#![deny(clippy::equatable_if_let)]
#![deny(clippy::needless_collect)]
#![deny(clippy::option_if_let_else)]
#![deny(clippy::useless_let_if_seq)]
#![deny(clippy::non_zero_suggestions)]
#![deny(clippy::string_lit_chars_any)]
#![deny(clippy::branches_sharing_code)]
#![deny(clippy::if_then_some_else_none)]
#![deny(clippy::set_contains_or_insert)]
#![deny(clippy::semicolon_outside_block)]
#![deny(clippy::needless_pass_by_ref_mut)]
#![deny(clippy::separated_literal_suffix)]
#![deny(clippy::empty_structs_with_brackets)]
#![deny(clippy::significant_drop_in_scrutinee)]
#![deny(clippy::empty_enum_variants_with_brackets)]
// Use tracing crate.
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]
// Expected
#![expect(clippy::multiple_crate_versions)]

//! Papo - GTK `WhatsApp` client.
//!
//! This is the main entry point for the Papo application.

// FIXME: Allow dead code for WIP features.
#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

#[rustfmt::skip]
mod config;
mod application;
mod components;
mod modals;
mod session;
mod state;
mod store;
mod utils;
mod widgets;

mod icon_names {
    #[allow(unused_imports)]
    pub use shipped::*; // Include all shipped icons by default
    include!(concat!(env!("OUT_DIR"), "/icon_names.rs"));
}

use std::{path::PathBuf, sync::LazyLock};

use gettextrs::LocaleCategory;
use gtk::{gio, glib, prelude::ApplicationExt};
use relm4::{RelmApp, gtk, main_application, once_cell::sync::OnceCell};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use application::Application;
use config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, PROFILE, RESOURCES_FILE, VERSION};

/// Papo's data directory path (e.g., ~/.local/share/papo on Linux).
pub static DATA_DIR: LazyLock<PathBuf> = LazyLock::new(|| glib::user_data_dir().join("papo"));

/// How many threads that Relm4 should use for asynchronous background tasks.
static RELM_THREADS: OnceCell<usize> = OnceCell::with_value(4);

relm4::new_action_group!(AppActionGroup, "app");
relm4::new_stateless_action!(QuitAction, AppActionGroup, "quit");

#[macro_export]
macro_rules! i18n {
    ($s:expr) => {
        gettextrs::gettext($s)
    };
}

#[macro_export]
macro_rules! i18n_f {
    ($s:expr, $($arg:tt)*) => {
        format!(gettextrs::gettext($s).as_str(), $($arg)*)
    };
}

#[macro_export]
macro_rules! ni18n {
    ($singular:expr, $plural:expr, $n:expr) => {
        gettextrs::ngettext($singular, $plural, $n)
    };
}

fn main() {
    // Initialize logger.
    // Default to the INFO level for this crate and WARN for everything else.
    // It can be overridden with the RUST_LOG environment variable.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("papo=info,warn"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(env_filter))
        .init();

    // Prepare i18n.
    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    glib::set_application_name("Papo");

    adw::init().expect("Failed to init GTK/libadwaita");

    let res = gio::Resource::load(RESOURCES_FILE).expect("Could not load gresource file");
    gio::resources_register(&res);

    gtk::Window::set_default_icon_name(APP_ID);

    let app = main_application();
    app.set_flags(gio::ApplicationFlags::HANDLES_OPEN);
    app.set_resource_base_path(Some("/com/amanoteam/Papo/"));

    let app = RelmApp::from_app(app);

    tracing::info!("Papo ({})", APP_ID);
    tracing::info!("Version: {} ({})", VERSION, PROFILE);

    let data = res
        .lookup_data(
            "/com/amanoteam/Papo/style.css",
            gio::ResourceLookupFlags::NONE,
        )
        .unwrap();
    relm4::set_global_css(&glib::GString::from_utf8_checked(data.to_vec()).unwrap());
    relm4_icons::initialize_icons(icon_names::GRESOURCE_BYTES, icon_names::RESOURCE_PREFIX);

    app.visible_on_activate(false).run_async::<Application>(());
}
