use std::{collections::HashMap, sync::Arc, time::Duration};

use adw::prelude::*;
use gtk::{gio, glib, pango};
use indexmap::IndexMap;
use relm4::{
    abstractions::Toaster,
    actions::{AccelsPlus, RelmAction, RelmActionGroup},
    main_application,
    prelude::*,
};
use strum::{AsRefStr, EnumString};
use tokio::time;
use wacore::types::message::MessageInfo;
use waproto::whatsapp::Message;

use crate::{
    components::{ChatList, ChatListInput, ChatListOutput, Login, LoginInput, LoginOutput},
    config::{APP_ID, PROFILE},
    i18n,
    modals::{about::AboutDialog, shortcuts::ShortcutsDialog},
    session::{Client, ClientInput, ClientOutput, RenderCache, RuntimeCache},
    state::{Chat, ChatMessage},
    store::Database,
    utils::extract_phone_from_jid,
};

pub struct Application {
    /// Page main stack is displaying.
    page: AppPage,
    /// User login component.
    login: AsyncController<Login>,
    /// Current app state.
    state: AppState,
    /// WhatsApp client wrapper.
    client: AsyncController<Client>,
    /// Toaster overlay.
    toaster: Toaster,
    /// Chat list data to avoid recomputation on every render.
    chat_list: AsyncController<ChatList>,
    /// Page session view is displaying.
    session_page: AppSessionPage,
    /// Currently selected chat JID.
    selected_chat: Option<String>,
    /// Progress bar displayed when syncing data.
    sync_progress_bar: gtk::ProgressBar,

    /// JID from the connected user.
    user_jid: Option<String>,
    /// Push name from the connected user.
    user_push_name: Option<String>,

    /// Papo's own database.
    db: Arc<Database>,
    /// Current chats data.
    chats: Vec<Chat>,
    /// UI render cache.
    render_cache: RenderCache,
    /// Runtime cache for WhatsApp data.
    runtime_cache: Arc<RuntimeCache>,
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

#[derive(Debug, PartialEq)]
enum AppState {
    /// Application is loading.
    Loading,

    /// Client is ready.
    Ready,
    /// Client is pairing.
    Pairing,
    /// Client is syncing.
    Syncing,
    /// Client is disconnected.
    Disconnected,

    /// Error state.
    Error(String),
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString, PartialEq)]
#[strum(serialize_all = "kebab-case")]
enum AppSessionPage {
    /// No chat selected view.
    Empty,
    /// Chat conversation view.
    Chat,
}

#[derive(Debug)]
pub enum AppMsg {
    /// User has been connected.
    Connected {
        jid: Option<String>,
        push_name: String,
    },
    /// Client has been logged out.
    LoggedOut,
    /// Reset the client session.
    ResetSession,
    /// Client has been disconnected.
    Disconnected,

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

    /// Read receipts updated.
    ReadReceipts {
        chat_jid: String,
        message_ids: Vec<String>,
    },

    /// New message received.
    MessageReceived {
        info: MessageInfo,
        message: Box<Message>,
    },

    Unknown,
    /// Error occurred.
    Error {
        message: String,
    },
    /// Quit the application.
    Quit,
}

#[derive(Debug)]
pub enum AppCmd {
    /// Sync cache from database.
    Sync,
}

impl Application {
    /// Add a chat.
    fn add_chat(&mut self, chat: Chat) {
        // Add chat to the cache.
        self.chats.push(chat.clone());

        // Sort: pinned first, then by last message time.
        self.chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| a.last_message_time.cmp(&b.last_message_time))
        });

        // Save to database in background.
        relm4::spawn({
            async move {
                if let Err(e) = chat.save().await {
                    tracing::error!("Failed to save chat: {}", e);
                }
            }
        });

        // Invalidate cache.
        self.render_cache.invalidate_chat_list();

        // Update chat list.
        let cached_chats = self.get_cached_chat_list();
        self.chat_list.emit(ChatListInput::Update {
            chats: cached_chats.to_vec(),
        });
    }

    /// Add a message to a chat.
    async fn add_message(&mut self, chat_jid: &str, message: ChatMessage) {
        // Check if the chat is a group.
        let is_group = chat_jid.ends_with("@g.us");

        // Find or create chat.
        let chat = if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) {
            chat
        } else {
            let name = if is_group {
                format!("{} {}", i18n!("Group"), &chat_jid[..8])
            } else {
                if self.user_jid.as_ref().is_some_and(|u_j| chat_jid == u_j) {
                    i18n!("You")
                } else {
                    message
                        .sender_name
                        .clone()
                        .unwrap_or_else(|| extract_phone_from_jid(chat_jid))
                }
            };

            // Create a new chat.
            self.add_chat(Chat {
                jid: chat_jid.to_string(),
                name,
                muted: false,
                pinned: false,
                unread_count: 0,
                participants: HashMap::new(),
                last_message_time: message.timestamp,

                db: Arc::clone(&self.db),
            });
            self.chats.last_mut().unwrap()
        };

        if !message.outgoing {
            // Increase unread count.
            chat.unread_count += 1;

            // Insert the sender in the chat's participants if they aren't there.
            if is_group && !chat.participants.contains_key(&message.sender_jid) {
                chat.participants.insert(
                    message.sender_jid.clone(),
                    message
                        .sender_name
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string()),
                );
            }
        }

        // Save to database in background.
        relm4::spawn({
            let chat = chat.clone();

            async move {
                if let Err(e) = chat.save().await {
                    tracing::error!("Failed to update chat: {}", e);
                }

                if let Err(e) = message.save().await {
                    tracing::error!("Failed to save message: {}", e);
                }
            }
        });

        // Invalidate cache.
        self.render_cache.invalidate_message_list(chat_jid);

        // Update chat list.
        let cached_chats = self.get_cached_chat_list();
        self.chat_list.emit(ChatListInput::Update {
            chats: cached_chats.to_vec(),
        });
    }

    /// Update application state.
    fn update_state(&mut self, state: AppState) {
        self.state = state;
    }

    /// Mark a chat as read.
    fn mark_chat_read(&mut self, chat_jid: &str) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) {
            chat.unread_count = 0;
            self.render_cache.invalidate_chat_list();
        }
    }

    /// Get cached chat list for UI rendering.
    fn get_cached_chat_list(&self) -> Arc<[Chat]> {
        self.render_cache.get_chat_list(&self.chats)
    }
}

relm4::new_action_group!(pub(super) WindowActionGroup, "win");
relm4::new_stateless_action!(ContactsAction, WindowActionGroup, "show-contacts");
relm4::new_stateless_action!(PreferencesAction, WindowActionGroup, "show-preferences");
relm4::new_stateless_action!(pub(super) ShortcutsAction, WindowActionGroup, "show-help-overlay");
relm4::new_stateless_action!(AboutAction, WindowActionGroup, "about");
relm4::new_stateless_action!(QuitAction, WindowActionGroup, "quit");

#[relm4::component(async, pub)]
impl AsyncComponent for Application {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Widgets = AppWidgets;
    type CommandOutput = AppCmd;

    menu! {
        primary_menu: {
            section! {
                "_Contacts" => ContactsAction,
            },
            section! {
                "_Preferences" => PreferencesAction,
                "_Keyboard Shortcuts" => ShortcutsAction,
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

                    add_named[Some("loading")] = &adw::ToolbarView {
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
                                set_label: &i18n!("Fetching account data..."),
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
                    },

                    #[local_ref]
                    add_named[Some("login")] = login_widget -> adw::ToolbarView {},

                    #[name = "split_view"]
                    add_named[Some("session")] = &adw::NavigationSplitView {
                        set_min_sidebar_width: 280.0,
                        set_max_sidebar_width: 350.0,

                        #[name = "sidebar"]
                        #[wrap(Some)]
                        set_sidebar = &adw::NavigationPage {
                            set_title: "Papo",
                            set_css_classes: &["background"],

                            #[wrap(Some)]
                            set_child = &adw::ToolbarView {
                                add_top_bar = &adw::HeaderBar {
                                    set_show_title: false,

                                    pack_start = &gtk::ToggleButton {
                                        set_css_classes: &["flat", "circular"],
                                        set_tooltip_text: Some(&i18n!("Your profile")),

                                        adw::Avatar {
                                            #[watch]
                                            set_text: Some(&if let Some(push_name) = model.user_push_name.clone() {
                                                push_name
                                            } else {
                                                i18n!("You")
                                            }),
                                            set_size: 30,
                                            set_show_initials: true,
                                        }
                                    },
                                    pack_end = &gtk::MenuButton {
                                        set_icon_name: "menu-symbolic",
                                        set_menu_model: Some(&primary_menu),
                                        set_tooltip_text: Some(&i18n!("Menu")),
                                    }
                                },
                                /* add_top_bar = &gtk::SearchEntry {
                                    set_margin_start: 8,
                                    set_margin_end: 8,
                                    set_margin_top: 4,
                                    set_margin_bottom: 12,
                                }, */

                                #[name = "view_stack"]
                                #[wrap(Some)]
                                set_content = &adw::ViewStack {
                                    #[local_ref]
                                    add_titled[Some("chats"), &i18n!("Chats")] = chat_list_widget -> gtk::ScrolledWindow {} -> {
                                        set_icon_name: Some("chat-bubbles-text-symbolic")
                                    },

                                    /* add_titled[Some("status"), &i18n!("Status")] = &gtk::ScrolledWindow {} -> {
                                        set_icon_name: Some("image-round-symbolic")
                                    } */
                                },

                                add_bottom_bar = &adw::ViewSwitcherBar {
                                    set_stack: Some(&view_stack),
                                    set_reveal: true
                                },
                            },
                        },

                        #[name = "content"]
                        #[wrap(Some)]
                        set_content = &adw::NavigationPage {
                            set_title: "Chat",
                            set_css_classes: &["view"],

                            #[wrap(Some)]
                            set_child = &adw::ToolbarView {
                                add_top_bar = &adw::HeaderBar {
                                    set_css_classes: &["flat"],
                                    #[watch]
                                    set_show_back_button: model.selected_chat.is_some(),

                                    #[name = "header_bar"]
                                    #[wrap(Some)]
                                    set_title_widget = &gtk::Button {
                                        set_halign: gtk::Align::Center,
                                        set_valign: gtk::Align::Center,
                                        set_css_classes: &["flat"],

                                        // connect_clicked => AppMsg::OpenChatProfile { ... },

                                        gtk::Box {
                                            set_halign: gtk::Align::Center,
                                            set_valign: gtk::Align::Center,
                                            set_orientation: gtk::Orientation::Vertical,

                                            gtk::Label {
                                                set_label: "", // TODO: show chat name
                                                #[watch]
                                                set_visible: model.selected_chat.is_some(),
                                                set_css_classes: &["title"]
                                            },

                                            gtk::Label {
                                                set_label: "A beautiful description", // TODO: show contact's status when chatting with a person and the chat's description when it's a group, channel or community
                                                #[watch]
                                                set_visible: model.selected_chat.is_some(),
                                                set_css_classes: &["dimmed"]
                                            }
                                        }
                                    }
                                },

                                #[wrap(Some)]
                                set_content = &gtk::Stack {
                                    set_transition_type: gtk::StackTransitionType::Crossfade,

                                    add_named[Some("empty")] = &adw::StatusPage {
                                        set_title: &i18n!("No Chat Selected"),
                                        set_hexpand: true,
                                        set_vexpand: true,
                                        set_can_focus: false,
                                        set_icon_name: Some("chat-bubbles-empty-symbolic"),
                                        set_description: Some(&i18n!("Select a chat to start chatting"))
                                    },

                                    #[watch]
                                    set_visible_child_name: model.session_page.as_ref(),
                                }
                            }
                        }
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
        let db = Arc::new(
            Database::new()
                .await
                .expect("Failed to initialize database"),
        );
        let runtime_cache = Arc::new(RuntimeCache::new());

        let login =
            Login::builder()
                .launch(())
                .forward(sender.input_sender(), |output| match output {
                    LoginOutput::ResetSession => AppMsg::ResetSession,

                    LoginOutput::PairWithPhoneNumber { phone_number } => {
                        AppMsg::PairWithPhoneNumber { phone_number }
                    }
                });

        let client = Client::builder()
            .launch(Arc::clone(&runtime_cache))
            .forward(sender.input_sender(), |output| match output {
                ClientOutput::Connected { jid, push_name } => AppMsg::Connected { jid, push_name },
                ClientOutput::LoggedOut => AppMsg::LoggedOut,
                ClientOutput::Disconnected => AppMsg::Disconnected,

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

                ClientOutput::ReadReceipts {
                    chat_jid,
                    message_ids,
                } => AppMsg::ReadReceipts {
                    chat_jid,
                    message_ids,
                },

                ClientOutput::MessageReceived { info, message } => {
                    AppMsg::MessageReceived { info, message }
                }

                ClientOutput::Error { message } => AppMsg::Error { message },
                _ => AppMsg::Unknown,
            });

        let chat_list = ChatList::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                ChatListOutput::ChatSelected(jid) => AppMsg::ChatSelected(jid),
            });

        let sync_progress_bar = gtk::ProgressBar::new();

        let model = Self {
            page: AppPage::Loading,
            login,
            state: AppState::Loading,
            client,
            toaster: Toaster::default(),
            chat_list,
            session_page: AppSessionPage::Empty,
            selected_chat: None,
            sync_progress_bar: sync_progress_bar.clone(),

            user_jid: None,
            user_push_name: None,

            db,
            chats: Vec::new(),
            render_cache: RenderCache::new(),
            runtime_cache,
        };

        let login_widget = model.login.widget();
        let toast_overlay = model.toaster.overlay_widget();
        let chat_list_widget = model.chat_list.widget();

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

        let widgets = view_output!();

        actions.add_action(shortcuts_action);
        actions.add_action(about_action);
        actions.add_action(quit_action);
        actions.register_for_widget(&widgets.main_window);

        widgets.load_window_size();

        AsyncComponentParts { model, widgets }
    }

    async fn update(
        &mut self,
        message: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            AppMsg::Connected { jid, push_name } => {
                self.user_jid = jid;
                self.user_push_name = Some(push_name);

                // Sync in background.
                relm4::spawn(async move {
                    sender.oneshot_command(async { AppCmd::Sync });
                });

                if self.page != AppPage::Session {
                    self.page = AppPage::Session;
                }
            }
            AppMsg::LoggedOut => {
                self.page = AppPage::Loading;
                self.update_state(AppState::Pairing);

                sender.input(AppMsg::ResetSession);

                // Clear database in background.
                let db = self.db.clone();
                let chats = self.chats.clone();
                relm4::spawn(async move {
                    for chat in chats {
                        let _ = db.delete_chat(&chat.jid).await; // Delete messages on cascade.
                    }
                });
            }
            AppMsg::Disconnected => {
                self.update_state(AppState::Disconnected);
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
                self.update_state(AppState::Syncing);
            }
            AppMsg::PairWithPhoneNumber { phone_number } => {
                self.client
                    .emit(ClientInput::PairWithPhoneNumber { phone_number });
            }

            AppMsg::ChatSelected(jid) => {
                self.selected_chat = Some(jid);
            }

            AppMsg::ReadReceipts {
                chat_jid,
                message_ids,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) {
                    for msg_id in message_ids {
                        if let Ok(Some(mut message)) = chat.find_message(&msg_id).await {
                            chat.unread_count -= 1;

                            message.unread = false;
                            if let Err(e) = message.save().await {
                                tracing::error!("Failed to update message: {}", e);
                            }
                        }
                    }

                    if let Err(e) = chat.save().await {
                        tracing::error!("Failed to update chat: {}", e);
                    }

                    // Update chat list.
                    let cached_chats = self.get_cached_chat_list();
                    self.chat_list.emit(ChatListInput::Update {
                        chats: cached_chats.to_vec(),
                    });
                }
            }

            AppMsg::MessageReceived { info, message } => {
                if let Some(content) = message.conversation.clone() {
                    match content.as_str() {
                        "status@broadcast" => {
                            // TODO: handle status events
                        }
                        _ if !content.is_empty() => {
                            let chat_jid = info.source.chat.to_string();
                            let outgoing = info.source.is_from_me;

                            let chat_message = ChatMessage {
                                id: info.id.clone(),
                                chat_jid: chat_jid.clone(),
                                sender_jid: info.source.sender.to_string(),
                                sender_name: Some(info.push_name.clone()),

                                media: None,
                                unread: !outgoing,
                                content,
                                outgoing,
                                reactions: IndexMap::new(),
                                timestamp: info.timestamp,

                                db: Arc::clone(&self.db),
                            };

                            self.add_message(&chat_jid, chat_message).await;
                        }
                        _ => {
                            tracing::trace!(
                                "Message received: info = {:#?}, message = {:#?}",
                                info,
                                message
                            );
                        }
                    }
                } else {
                    if let Some(sent_message) = message.device_sent_message {
                        if let Some(_chat_jid) = sent_message.destination_jid {
                            if let Some(msg) = sent_message.message {
                                if let Some(_reaction) = msg.reaction_message {
                                    // TODO: handle
                                } else if let Some(_sticker) = msg.sticker_message {
                                    // TODO: handle
                                }
                            }
                        } else {
                            // TODO: maybe add message to "You" chat?
                        }
                    } else {
                        tracing::trace!(
                            "Message without content received: info = {:#?}, message = {:#?}",
                            info,
                            message
                        );
                    }
                }
            }

            AppMsg::Unknown => {}
            AppMsg::Error { message } => {
                self.update_state(AppState::Error(message.clone()));

                match self.page {
                    AppPage::Login => {
                        self.login.emit(LoginInput::Error { message });
                    }
                    AppPage::Loading => {
                        self.page = AppPage::Error;
                    }
                    AppPage::Session => {
                        // TODO: display error
                    }
                    AppPage::Error => {}
                }
            }
            AppMsg::Quit => main_application().quit(),
        }
    }

    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        _sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            AppCmd::Sync => {
                self.update_state(AppState::Syncing);

                // Load chats.
                match self.db.load_chats().await {
                    Ok(chats) => {
                        tracing::info!("Loaded {} chats from own database", chats.len());
                        self.chats.extend(chats);

                        // Update chat list.
                        let cached_chats = self.get_cached_chat_list();
                        self.chat_list.emit(ChatListInput::Update {
                            chats: cached_chats.to_vec(),
                        });
                    }
                    Err(e) => tracing::error!("Failed to load chats from own database: {}", e),
                }

                self.update_state(AppState::Ready);
            }
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
