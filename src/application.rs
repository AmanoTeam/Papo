use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib, pango};
use relm4::{
    abstractions::Toaster,
    actions::{AccelsPlus,  RelmAction, RelmActionGroup},
    main_application,
    prelude::*,
};
use strum::{AsRefStr, EnumString};
use tokio::time;

use crate::{
    client::{Client, ClientInput, ClientOutput},
    components::{ChatList, ChatListOutput, Login, LoginInput, LoginOutput},
    config::{APP_ID, PROFILE},
    i18n,
    modals::{about::AboutDialog, shortcuts::ShortcutsDialog},
};

pub struct Application {
    /// Page app is displaying.
    page: AppPage,
    /// User login component.
    login: AsyncController<Login>,
    /// WhatsApp client wrapper.
    client: AsyncController<Client>,
    /// Toaster overlay.
    toaster: Toaster,
    /// Chat list data to avoid recomputation on every render.
    chat_list: AsyncController<ChatList>,
    /// Currently selected chat JID.
    selected_chat: Option<String>,
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString, PartialEq)]
#[strum(serialize_all = "lowercase")]
enum AppPage {
    /// Loading page.
    Loading,
    /// Login view.
    Login,
    /// Session view.
    Session,
    /// Error page.
    Error,
}

#[derive(Debug)]
pub enum AppMsg {
    /// User has been connected.
    Connected,
    /// Reset the client session.
    ResetSession,

    /// Pair device.
    PairDevice {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Device has successfully paired.
    DevicePaired,
    /// Pair with a phone number.
    PairWithPhoneNumber {
        phone_number: String,
    },

    /// Select a chat.
    ChatSelected(String),

    Unknown,
    /// Error occurred.
    Error { message: String },
    /// Quit the application.
    Quit,
}

relm4::new_action_group!(pub(super) WindowActionGroup, "win");
relm4::new_stateless_action!(PreferencesAction, WindowActionGroup, "preferences");
relm4::new_stateless_action!(pub(super) ShortcutsAction, WindowActionGroup, "show-help-overlay");
relm4::new_stateless_action!(AboutAction, WindowActionGroup, "about");
relm4::new_stateless_action!(QuitAction, WindowActionGroup, "quit");

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for Application {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Widgets = AppWidgets;

    menu! {
        primary_menu: {
            section! {
                "_Preferences" => PreferencesAction,
                "_Keyboard" => ShortcutsAction,
                "_About Papo" => AboutAction,
            }
        }
    }

    view! {
        #[root]
        main_window = adw::ApplicationWindow::new(&main_application()) {
            set_title: Some("Papo"),
            set_visible: true,
            set_width_request: 400,
            set_height_request: 360,
            set_default_width: 900,
            set_default_height: 850,

            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::Quit);
                glib::Propagation::Stop
            },

            add_css_class?: if PROFILE == "Devel" {
                Some("devel")
            } else {
                None
            },

            #[local_ref]
            toast_overlay -> adw::ToastOverlay {
                #[name = "main_stack"]
                gtk::Stack {
                    set_transition_type: gtk::StackTransitionType::Crossfade,

                    add_child = &adw::ToolbarView {
                        add_top_bar = &adw::HeaderBar {
                            pack_end = &gtk::Button {
                                set_icon_name: "info-outline-symbolic",
                                set_action_name: Some("win.about"),
                                set_tooltip_text: Some(&i18n!("About Papo")),
                            }
                        },

                        gtk::Box {
                            set_halign: gtk::Align::Center,
                            set_valign: gtk::Align::Center,
                            set_vexpand: true,
                            set_spacing: 24,
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Label {
                                set_label: &i18n!("Fething account data..."),
                                set_halign: gtk::Align::Center,
                                set_justify: gtk::Justification::Center,
                                set_css_classes: &["title-2"],

                                set_wrap: true,
                                set_wrap_mode: pango::WrapMode::WordChar
                            },

                            adw::Spinner {
                                set_width_request: 48,
                                set_height_request: 48
                            }
                        }
                    } -> {
                        set_name: "loading"
                    },

                    #[local_ref]
                    add_child = &login_widget -> adw::ToolbarView {} -> {
                        set_name: "login"
                    },

                    #[name = "split_view"]
                    add_child = &adw::NavigationSplitView {
                        #[name = "sidebar"]
                        #[wrap(Some)]
                        set_sidebar = &adw::NavigationPage {
                            set_title: "Papo",

                            #[wrap(Some)]
                            set_child = &adw::ToolbarView {
                                add_top_bar = &adw::HeaderBar {
                                    set_show_title: false,

                                    pack_start = &gtk::Button {
                                        set_css_classes: &["flat", "circular"],
                                        set_tooltip_text: Some(&i18n!("Your profile")),

                                        adw::Avatar {
                                            set_text: Some(&i18n!("You")),
                                            set_size: 32,
                                            set_show_initials: true,
                                        }
                                    },
                                    pack_end = &gtk::Button {
                                        set_icon_name: "list-add-symbolic",
                                        set_tooltip_text: Some(&i18n!("New Chat")),
                                    },
                                    pack_end = &gtk::MenuButton {
                                        set_icon_name: "open-menu-symbolic",
                                        set_menu_model: Some(&primary_menu),
                                        set_tooltip_text: Some(&i18n!("Menu")),
                                    }
                                },
                                add_top_bar = &gtk::SearchEntry {
                                    set_margin_start: 12,
                                    set_margin_end: 12,
                                    set_margin_top: 6,
                                    set_margin_bottom: 6,

                                    set_css_classes: &["sidebar-search"],
                                    set_placeholder_text: Some(&i18n!("Search chats...")),
                                },

                                #[name = "view_stack"]
                                #[wrap(Some)]
                                set_content = &adw::ViewStack {
                                    // add_titled: () -> {}
                                },

                                add_bottom_bar = &adw::ViewSwitcherBar {
                                    set_reveal: true,

                                    // set_stack = &view_stack,
                                },
                            },
                        },

                        #[wrap(Some)]
                        set_content = &adw::NavigationPage {
                            set_title: "Chat",

                            #[wrap(Some)]
                            set_child = &adw::ToolbarView {
                                add_top_bar = &adw::HeaderBar {},

                                // set_content = {}
                            }
                        }
                    } -> {
                        set_name: "session"
                    },

                    #[watch]
                    set_visible_child_name: model.page.as_ref(),
                },
            },

            add_breakpoint = bp_with_setters(
                adw::Breakpoint::new(
                    adw::BreakpointCondition::new_length(
                        adw::BreakpointConditionLengthType::MaxWidth,
                        600.0,
                        adw::LengthUnit::Sp,
                    )
                ),
                &[(&split_view, "collapsed", true)]
            ),
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let login = Login::builder().launch(()).forward(sender.input_sender(), |output| match output {
            LoginOutput::ResetSession => AppMsg::ResetSession,

            LoginOutput::PairWithPhoneNumber { phone_number } => AppMsg::PairWithPhoneNumber { phone_number },
        });

        let client = Client::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                ClientOutput::Connected => AppMsg::Connected,

                ClientOutput::PairCode {
                    code,
                    qr_code,
                    timeout,
                } => AppMsg::PairDevice {
                    code,
                    qr_code,
                    timeout,
                },
                ClientOutput::PairSuccess => AppMsg::DevicePaired,

                ClientOutput::Error { message } => AppMsg::Error { message },
                _ => AppMsg::Unknown,
            });

        let chat_list = ChatList::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                ChatListOutput::ChatSelected(id) => AppMsg::ChatSelected(id),
            });

        let login_widget = login.widget().clone();

        let model = Self {
            page: AppPage::Loading,
            login,
            client,
            toaster: Toaster::default(),
            chat_list,
            selected_chat: None,
        };

        let toast_overlay = model.toaster.overlay_widget();
        let widgets = view_output!();

        let app = root.application().unwrap();
        let mut actions = RelmActionGroup::<WindowActionGroup>::new();

        let shortcuts_action = {
            RelmAction::<ShortcutsAction>::new_stateless(move |_| {
                ShortcutsDialog::builder().launch(()).detach();
            })
        };

        let about_action = {
            RelmAction::<AboutAction>::new_stateless(move |_| {
                AboutDialog::builder().launch(()).detach();
            })
        };

        let quit_action = {
            let sender = sender.clone();
            RelmAction::<QuitAction>::new_stateless(move |_| {
                sender.input(AppMsg::Quit);
            })
        };

        // Connect actions with hotkeys
        app.set_accelerators_for_action::<QuitAction>(&["<Control>q"]);
        // app.set_accelerators_for_action::<QuitAction>(&["<Control>w"]);

        actions.add_action(shortcuts_action);
        actions.add_action(about_action);
        actions.add_action(quit_action);
        actions.register_for_widget(&widgets.main_window);

        widgets.load_window_size();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, message: Self::Input, _sender: AsyncComponentSender<Self>) {
        match message {
            AppMsg::Connected => {
                self.page = AppPage::Session;
            }
            AppMsg::ResetSession => {
                self.client.emit(ClientInput::Restart);
            }

            AppMsg::PairDevice {
                code,
                qr_code,
                timeout,
            } => {
                if self.page == AppPage::Loading {
                    self.page = AppPage::Login;
                }

                self.login.emit(LoginInput::PairCode {
                    code,
                    qr_code,
                    timeout,
                });
            }
            AppMsg::DevicePaired => {
                self.login.emit(LoginInput::PairSuccess);

                time::sleep(Duration::from_secs(1)).await;
                self.page = AppPage::Session;
            }
            AppMsg::PairWithPhoneNumber { phone_number } => {
                self.client.emit(ClientInput::PairWithPhoneNumber { phone_number });
            }

            AppMsg::ChatSelected(_id) => {}

            AppMsg::Unknown => {}
            AppMsg::Error { message } => {
                if self.page == AppPage::Login {
                    self.login.emit(LoginInput::Error { message });
                } else {
                    self.page = AppPage::Error;
                    // TODO: show error
                }
            }
            AppMsg::Quit => main_application().quit(),
        }
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        widgets.save_window_size().unwrap();
    }
}

impl AppWidgets {
    fn load_window_size(&self) {
        let settings = gio::Settings::new(APP_ID);

        let width = settings.int("window-width");
        let height = settings.int("window-height");
        let is_maximized = settings.boolean("is-maximized");

        self.main_window.set_default_size(width, height);

        if is_maximized {
            self.main_window.maximize();
        }
    }

    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let settings = gio::Settings::new(APP_ID);
        let (width, height) = self.main_window.default_size();

        settings.set_int("window-width", width)?;
        settings.set_int("window-height", height)?;

        settings.set_boolean("is-maximized", self.main_window.is_maximized())?;

        Ok(())
    }
}

fn bp_with_setters(
    bp: adw::Breakpoint,
    additions: &[(&impl IsA<glib::Object>, &str, impl ToValue)],
) -> adw::Breakpoint {
    bp.add_setters(additions);
    bp
}
