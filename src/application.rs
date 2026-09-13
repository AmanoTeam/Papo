use std::{collections::HashMap, time::Duration};

use adw::{NavigationSplitView, prelude::*};
use gtk::{gio, glib};
use indexmap::IndexMap;
use jiff::Timestamp;
use relm4::{
    abstractions::Toaster,
    actions::{AccelsPlus, RelmAction, RelmActionGroup},
    main_application,
    prelude::*,
};
use strum::{AsRefStr, EnumString};
use tokio::time;
use uuid::Uuid;
use whatsapp_rust::{
    types::{message::MessageInfo, presence::ReceiptType},
    waproto::whatsapp::Message,
};

use crate::{
    DATA_DIR,
    components::{
        ChatList, ChatListInput, ChatListOutput, ChatView, ChatViewInput, ChatViewOutput, Login,
        LoginInput, LoginOutput, Welcome, WelcomeOutput,
    },
    config::{APP_ID, PROFILE},
    db::{
        entities::{Contact, Session},
        keyring::KeyringService,
        session::open_session_db,
        session_manager::SessionManager,
        store::SessionStore,
    },
    i18n,
    modals::{about::AboutDialog, shortcuts::ShortcutsDialog},
    session::{ChatsSyncedEntry, Client, ClientInput, ClientOutput},
    state::{Chat, ChatMessage, MessageStatus, TypingSender},
    utils::{format_lid_as_number, get_first_name},
};

pub struct Application {
    db: SessionStore,
    page: AppPage,
    chats: Vec<Chat>,
    login: AsyncController<Login>,
    state: AppState,
    client: AsyncController<Client>,
    sender: AsyncComponentSender<Self>,
    typing: HashMap<String, ChatTypingState>,
    session: Session,
    /// Resolved contact list (JID -> name).
    contacts: HashMap<String, String>,

    toaster: Toaster,
    user_jid: Option<String>,
    welcome: AsyncController<Welcome>,
    chat_list: AsyncController<ChatList>,
    chat_view: AsyncController<ChatView>,
    split_view: NavigationSplitView,
    session_page: AppSessionPage,
    user_push_name: Option<String>,
}

#[derive(Clone, Copy, Debug, AsRefStr, PartialEq, EnumString)]
#[strum(serialize_all = "lowercase")]
enum AppPage {
    Login,
    Session,
    Welcome,
    Error,
}

#[derive(Debug, PartialEq)]
enum AppState {
    Loading,

    Ready,
    Pairing,
    Syncing,
    Disconnected,

    Error(String),
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString, PartialEq)]
#[strum(serialize_all = "kebab-case")]
enum AppSessionPage {
    Empty,
    ChatHistory,
}

#[derive(Debug)]
pub enum AppMsg {
    Connected {
        jid: Option<String>,
        push_name: String,
    },
    LoggedOut,
    ResetSession,
    Disconnected,
    SelfPushNameUpdated {
        push_name: String,
    },

    PairDevice {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    DevicePaired,
    PairWithPhoneNumber {
        phone_number: String,
    },
    SwitchToLoginQrCode,
    SwitchToLoginPhoneNumber,

    ChatOpen,
    ChatClosed,
    ChatSelected(String),
    MarkChatRead(String),

    AvatarUpdate {
        jid: String,
        path: String,
    },
    /// Contact updated (from sync or individual update).
    ContactUpdate {
        jid: String,
        name: Option<String>,
        push_name: Option<String>,
        phone_number: String,
    },
    ReceiptUpdate {
        chat_jid: String,
        message_ids: Vec<String>,
        receipt_type: ReceiptType,
    },
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<Timestamp>,
    },
    ChatPresenceUpdate {
        chat_jid: String,
        active: bool,
        recording: bool,
        sender_jid: String,
        sender_alt: Option<String>,
    },
    MessageStatusUpdate {
        chat_jid: String,
        msg_id: Uuid,
        status: MessageStatus,
    },

    LidPnResolved {
        chat_jid: String,
        lid: String,
        phone: Option<String>,
    },
    TypingExpired {
        chat_jid: String,
        sender_jid: String,
        generation: u64,
    },
    TypingStateChanged {
        chat_jid: String,
        composing: bool,
    },

    MessageReceived {
        info: Box<MessageInfo>,
        message: Box<Message>,
    },

    SendTextMessage {
        text: String,
        recipient: String,
    },

    ChatsSynced {
        entries: Vec<ChatsSyncedEntry>,
    },
    SyncCompleted {
        chats_needing_avatars: Vec<String>,
    },

    ChatPropertyUpdate {
        jid: String,
        pinned: Option<bool>,
        muted: Option<bool>,
        archived: Option<bool>,
    },
    HistorySyncCompleted,
    OfflineSyncCompleted,

    Unknown,
    Error {
        message: String,
    },
    Quit,
}

#[derive(Debug)]
pub enum AppCmd {
    /// Sync cache from database.
    LoadCache,
    SwitchSession {
        db: SessionStore,
        session: Session,
    },

    SyncChats {
        entries: Vec<ChatsSyncedEntry>,
    },

    UpdateChat {
        chat: Chat,
        move_to_top: bool,
    },
    AddChatToList {
        chat: Chat,
    },
}

#[derive(Debug, Default)]
struct ChatTypingState {
    senders: IndexMap<String, TypingSender>,
    generation: u64,
}

impl ChatTypingState {
    fn typing_senders(&self) -> Vec<TypingSender> {
        self.senders.values().cloned().collect()
    }
}

impl Application {
    fn add_chat(&mut self, mut chat: Chat) {
        if let Some(name) = self.contacts.get(&chat.jid).filter(|n| !n.is_empty()) {
            chat.name.clone_from(name);
        }

        // Insert the chat into our cached list.
        self.chats.push(chat.clone());

        // Sort all our chats.
        self.chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.last_message_time.cmp(&a.last_message_time))
        });

        // Save the chat in the database.
        let chat_clone = chat.clone();
        relm4::spawn(async move {
            if let Err(e) = chat_clone.upsert().await {
                tracing::error!("Failed to save chat: {}", e);
            }
        });

        // Add the chat in the chat list.
        self.chat_list
            .emit(ChatListInput::AddChat { chat, at_top: true });
    }

    fn emit_typing(&self, chat_jid: &str) {
        let Some(state) = self.typing.get(chat_jid) else {
            return;
        };

        let senders = state.typing_senders();
        self.chat_list.emit(ChatListInput::UpdateTyping {
            chat_jid: chat_jid.to_string(),
            senders: senders.clone(),
        });

        self.chat_view.emit(ChatViewInput::TypingUpdate {
            chat_jid: chat_jid.to_string(),
            senders,
        });
    }

    fn register_participant(&mut self, chat_jid: &str, jid: &str, name: Option<&str>) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
            && chat.is_group()
        {
            let entry = chat
                .participants
                .entry(jid.to_string())
                .or_insert_with(|| i18n!("Unknown"));
            if let Some(name) = name.filter(|n| !n.is_empty())
                && *entry == i18n!("Unknown")
            {
                *entry = name.to_string();
            }
        }
    }

    fn add_message(&mut self, chat_jid: &str, message: ChatMessage) {
        // Check if the message's chat is a group.
        let is_group = chat_jid.ends_with("@g.us");

        // Create a new chat if it doesn't exists.
        if !self.chats.iter().any(|c| c.jid == chat_jid) {
            let name = if is_group {
                format!("{} {}", i18n!("Group"), &chat_jid[..8])
            } else if self.user_jid.as_ref().is_some_and(|u_j| chat_jid == u_j) {
                i18n!("You")
            } else {
                message
                    .sender_name
                    .clone()
                    .unwrap_or_else(|| format_lid_as_number(chat_jid))
            };

            self.add_chat(Chat {
                jid: chat_jid.to_string(),
                name,
                muted: false,
                pinned: false,
                archived: false,
                available: None,
                last_seen: None,
                avatar_path: None,
                participants: HashMap::new(),
                last_message_time: message.timestamp,

                db: self.db.clone(),
            });

            self.client.emit(ClientInput::FetchAvatar {
                jid: chat_jid.to_string(),
            });
        }

        let typing_cleared = !message.outgoing
            && self
                .typing
                .get_mut(chat_jid)
                .is_some_and(|state| state.senders.shift_remove(&message.sender_jid).is_some());

        // Get the chat.
        let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) else {
            return;
        };

        // Check if the message was sent by the connected user.
        if !message.outgoing && is_group && !chat.participants.contains_key(&message.sender_jid) {
            chat.participants.insert(
                message.sender_jid.clone(),
                message
                    .sender_name
                    .clone()
                    .unwrap_or_else(|| i18n!("Unknown")),
            );
        }

        // Save the chat in the database.
        let chat_clone = chat.clone();
        relm4::spawn(async move {
            if let Err(e) = chat_clone.upsert().await {
                tracing::error!("Failed to update chat: {}", e);
            }
        });

        self.chat_view
            .emit(ChatViewInput::MessageReceived(Box::new(message.clone())));

        // Save the message in the database, then update the chat list.
        let sender = self.sender.clone();
        let chat_clone = chat.clone();
        sender.oneshot_command(async move {
            if let Err(e) = message.upsert().await {
                tracing::error!("Failed to save message: {}", e);
            }

            AppCmd::UpdateChat {
                chat: chat_clone,
                move_to_top: true,
            }
        });

        if typing_cleared {
            self.emit_typing(chat_jid);
        }
    }

    async fn mark_chat_read(&mut self, chat_jid: &str) {
        // Find the chat.
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) {
            // Collect unread messages before marking them as read locally.
            let messages = chat.get_unread_messages().await.unwrap_or_default();

            // Separate messages by sender.
            let mut sender_messages: IndexMap<String, Vec<String>> = IndexMap::new();
            for message in messages {
                let sender_jid = message.sender_jid;

                sender_messages
                    .entry(sender_jid)
                    .or_default()
                    .push(message.server_id);
            }

            // Send read receipts to WhatsApp.
            for (sender_jid, message_ids) in sender_messages {
                self.client.emit(ClientInput::MarkRead {
                    chat_jid: chat_jid.to_string(),
                    sender_jid: Some(sender_jid),
                    message_ids,
                });
            }

            // Mark chat as read locally, then update the chat list.
            let sender = self.sender.clone();
            let chat_clone = chat.clone();
            sender.oneshot_command(async move {
                if let Err(e) = chat_clone.mark_read().await {
                    tracing::error!("Failed to mark a chat as read: {e}");
                }

                AppCmd::UpdateChat {
                    chat: chat_clone,
                    move_to_top: false,
                }
            });
        }
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
                &i18n!("_Contacts") => ContactsAction,
            },
            section! {
                &i18n!("_Preferences") => PreferencesAction,
                &i18n!("_Keyboard Shortcuts") => ShortcutsAction,
                &i18n!("_About Papo") => AboutAction,
            }
        }
    }

    view! {
        #[root]
        main_window = adw::ApplicationWindow::new(&main_application()) {
            set_title: Some(&i18n!("Papo")),
            set_visible: true,
            set_width_request: 360,
            set_height_request: 440,
            set_default_width: 900,
            set_default_height: 850,

            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::Quit);
                glib::Propagation::Stop
            },

            add_css_class?: (PROFILE == "Devel").then_some("devel"),

            #[local_ref]
            toast_overlay -> adw::ToastOverlay {
                #[name = "main_stack"]
                gtk::Stack {
                    set_transition_type: gtk::StackTransitionType::Crossfade,

                    #[local_ref]
                    add_named[Some("welcome")] = welcome_widget -> adw::ToolbarView {},

                    #[local_ref]
                    add_named[Some("login")] = login_widget -> adw::ToolbarView {},

                    add_named[Some("session")] = &adw::BreakpointBin {
                        set_width_request: main_window.width_request(),
                        set_height_request: main_window.height_request(),

                        #[local_ref]
                        #[wrap(Some)]
                        set_child = split_view -> adw::NavigationSplitView {
                            set_min_sidebar_width: 280.0,
                            set_max_sidebar_width: 360.0,

                            #[name = "sidebar"]
                            #[wrap(Some)]
                            set_sidebar = &adw::NavigationPage {
                                set_title: &i18n!("Papo"),
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
                                                set_text: Some(&model.user_push_name.clone().unwrap_or_else(|| i18n!("You"))),
                                                set_size: 30,
                                                set_show_initials: true,
                                            }
                                        },
                                        pack_end = &gtk::MenuButton {
                                            set_icon_name: "menu-symbolic",
                                            set_menu_model: Some(&primary_menu),
                                            set_tooltip_text: Some(&i18n!("Menu")),
                                        },
                                    },

                                    #[name = "view_stack"]
                                    #[wrap(Some)]
                                    set_content = &adw::ViewStack {
                                        #[local_ref]
                                        add_titled[Some("chats"), &i18n!("Chats")] = chat_list_widget -> gtk::Box {} -> {
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
                                set_title: &i18n!("Chat"),
                                set_css_classes: &["view"],

                                #[wrap(Some)]
                                set_child = &gtk::Stack {
                                    set_transition_type: gtk::StackTransitionType::Crossfade,

                                    add_named[Some("empty")] = &adw::StatusPage {
                                        set_title: &i18n!("No Chat Selected"),
                                        set_hexpand: true,
                                        set_vexpand: true,
                                        set_can_focus: false,
                                        set_icon_name: Some("chat-bubbles-empty-symbolic"),
                                        set_description: Some(&i18n!("Select a chat to start chatting"))
                                    },

                                    #[local_ref]
                                    add_named[Some("chat-history")] = chat_view_widget -> adw::ToolbarView {},

                                    #[watch]
                                    set_visible_child_name: model.session_page.as_ref(),
                                }
                            }
                        },

                        add_breakpoint = bp_with_setters(
                            adw::Breakpoint::new(
                                adw::BreakpointCondition::new_length(
                                    adw::BreakpointConditionLengthType::MaxWidth,
                                    600.0,
                                    adw::LengthUnit::Sp,
                                )
                            ),
                            &[(split_view, "collapsed", true)]
                        ),
                    },

                    #[watch]
                    set_visible_child_name: model.page.as_ref(),
                },
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let keyring = KeyringService::new()
            .await
            .expect("Failed to access keyring");
        let mut session_manager = SessionManager::new(keyring)
            .await
            .expect("Failed to open main database");
        let session = match session_manager.last_valid_session().await {
            Ok(Some(session)) => session,
            _ => session_manager
                .create_session()
                .await
                .expect("Failed to create session"),
        };

        let db = open_session_db(&session.uuid, session_manager.keyring())
            .await
            .expect("Failed to open session database");
        let db = SessionStore::new(db, &session.uuid);

        let login =
            Login::builder()
                .launch(())
                .forward(sender.input_sender(), |output| match output {
                    LoginOutput::ResetSession => AppMsg::ResetSession,

                    LoginOutput::PairWithPhoneNumber { phone_number } => {
                        AppMsg::PairWithPhoneNumber { phone_number }
                    }
                });

        let mut contacts = HashMap::new();
        if let Ok(stored) = db.get_all_contacts().await {
            for contact in stored {
                let Contact {
                    jid,
                    name,
                    push_name,
                    ..
                } = contact;
                let display = name
                    .filter(|n| !n.is_empty())
                    .or_else(|| push_name.filter(|n| !n.is_empty()));

                if let Some(display) = display {
                    contacts.insert(jid, display);
                }
            }
        }

        let client =
            Client::builder()
                .launch(db.clone())
                .forward(sender.input_sender(), |output| match output {
                    ClientOutput::Connected { jid, push_name } => {
                        AppMsg::Connected { jid, push_name }
                    }
                    ClientOutput::LoggedOut => AppMsg::LoggedOut,
                    ClientOutput::Disconnected => AppMsg::Disconnected,
                    ClientOutput::SelfPushNameUpdated { push_name } => {
                        AppMsg::SelfPushNameUpdated { push_name }
                    }

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

                    ClientOutput::ReceiptUpdate {
                        chat_jid,
                        message_ids,
                        receipt_type,
                    } => AppMsg::ReceiptUpdate {
                        chat_jid,
                        message_ids,
                        receipt_type,
                    },
                    ClientOutput::PresenceUpdate {
                        jid,
                        available,
                        last_seen,
                    } => AppMsg::PresenceUpdate {
                        jid,
                        available,
                        last_seen,
                    },
                    ClientOutput::ChatPresenceUpdate {
                        chat_jid,
                        active,
                        recording,
                        sender_jid,
                        sender_alt,
                    } => AppMsg::ChatPresenceUpdate {
                        chat_jid,
                        active,
                        recording,
                        sender_jid,
                        sender_alt,
                    },

                    ClientOutput::MessageReceived { info, message } => {
                        AppMsg::MessageReceived { info, message }
                    }
                    ClientOutput::MessageSent { chat_jid, msg_id } => AppMsg::MessageStatusUpdate {
                        chat_jid,
                        msg_id,
                        status: MessageStatus::Sent,
                    },
                    ClientOutput::MessageFailed { chat_jid, msg_id } => {
                        AppMsg::MessageStatusUpdate {
                            chat_jid,
                            msg_id,
                            status: MessageStatus::Failed,
                        }
                    }

                    ClientOutput::ChatsSynced { entries } => AppMsg::ChatsSynced { entries },

                    ClientOutput::ChatPropertyUpdate {
                        jid,
                        pinned,
                        muted,
                        archived,
                    } => AppMsg::ChatPropertyUpdate {
                        jid,
                        pinned,
                        muted,
                        archived,
                    },

                    ClientOutput::HistorySyncCompleted => AppMsg::HistorySyncCompleted,
                    ClientOutput::OfflineSyncCompleted => AppMsg::OfflineSyncCompleted,

                    ClientOutput::AvatarUpdate { jid, path } => AppMsg::AvatarUpdate { jid, path },
                    ClientOutput::ContactUpdate {
                        jid,
                        name,
                        push_name,
                        phone_number,
                    } => AppMsg::ContactUpdate {
                        jid,
                        name,
                        push_name,
                        phone_number,
                    },

                    ClientOutput::LidPnResolved {
                        chat_jid,
                        lid,
                        phone,
                    } => AppMsg::LidPnResolved {
                        chat_jid,
                        lid,
                        phone,
                    },

                    ClientOutput::Error { message } => AppMsg::Error { message },
                    _ => AppMsg::Unknown,
                });

        let welcome = Welcome::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                WelcomeOutput::PairWithQrCode => AppMsg::SwitchToLoginQrCode,
                WelcomeOutput::PairWithPhoneNumber => AppMsg::SwitchToLoginPhoneNumber,
            });
        let chat_list = ChatList::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                ChatListOutput::ChatSelected(jid) => AppMsg::ChatSelected(jid),
            });
        let chat_view = ChatView::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                ChatViewOutput::ChatOpen => AppMsg::ChatOpen,
                ChatViewOutput::ChatClosed => AppMsg::ChatClosed,
                ChatViewOutput::MarkChatRead(jid) => AppMsg::MarkChatRead(jid),

                ChatViewOutput::SendTextMessage { text, recipient } => {
                    AppMsg::SendTextMessage { text, recipient }
                }

                ChatViewOutput::TypingStateChanged {
                    chat_jid,
                    composing,
                } => AppMsg::TypingStateChanged {
                    chat_jid,
                    composing,
                },
            });

        let model = Self {
            db,
            page: AppPage::Welcome,
            chats: Vec::new(),
            login,
            state: AppState::Loading,
            client,
            sender: sender.clone(),
            typing: HashMap::new(),
            session,
            toaster: Toaster::default(),
            contacts,
            user_jid: None,
            welcome,
            chat_list,
            chat_view,
            split_view: NavigationSplitView::new(),
            session_page: AppSessionPage::Empty,
            user_push_name: None,
        };

        let split_view = &model.split_view;
        let login_widget = model.login.widget();
        let welcome_widget = model.welcome.widget();
        let toast_overlay = model.toaster.overlay_widget();
        let chat_list_widget = model.chat_list.widget();
        let chat_view_widget = model.chat_view.widget();

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

    #[allow(clippy::too_many_lines)]
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
                sender.oneshot_command(async { AppCmd::LoadCache });

                if self.page != AppPage::Session {
                    self.page = AppPage::Session;
                }
            }

            AppMsg::SyncCompleted {
                chats_needing_avatars,
            } => {
                // Fetch avatars for chats that don't have them.
                for jid in chats_needing_avatars {
                    self.client.emit(ClientInput::FetchAvatar { jid });
                }
            }
            AppMsg::LoggedOut => {
                self.page = AppPage::Welcome;
                self.state = AppState::Pairing;
                self.chats.clear();

                // Start a fresh client — the old credentials have been cleared
                // by the ClientCommand::LoggedOut handler.
                self.client.emit(ClientInput::Start);

                let session_uuid = self.session.uuid.clone();
                sender.oneshot_command(async move {
                    let keyring = KeyringService::new()
                        .await
                        .expect("Failed to access keyring");
                    let mut session_manager = SessionManager::new(keyring)
                        .await
                        .expect("Failed to open main database");
                    session_manager
                        .delete_session(&session_uuid)
                        .await
                        .expect("Failed to delete session");

                    let session = session_manager
                        .create_session()
                        .await
                        .expect("Failed to create session");
                    let db = open_session_db(&session.uuid, session_manager.keyring())
                        .await
                        .expect("Failed to open session database");

                    AppCmd::SwitchSession {
                        db: SessionStore::new(db, &session.uuid),
                        session,
                    }
                });
            }
            AppMsg::Disconnected => {
                self.state = AppState::Disconnected;
            }
            AppMsg::SelfPushNameUpdated { push_name } => {
                self.user_push_name = Some(push_name);
            }
            AppMsg::ResetSession => {
                self.client.emit(ClientInput::Restart);
            }

            AppMsg::PairDevice {
                code,
                qr_code,
                timeout,
            } => {
                self.login.emit(LoginInput::PairCode {
                    code,
                    qr_code,
                    timeout,
                });
            }
            AppMsg::DevicePaired => {
                self.login.emit(LoginInput::PairSuccess);
                time::sleep(Duration::from_secs(2)).await;

                self.page = AppPage::Session;
                self.state = AppState::Syncing;
            }
            AppMsg::PairWithPhoneNumber { phone_number } => {
                self.client
                    .emit(ClientInput::PairWithPhoneNumber { phone_number });
            }

            AppMsg::SwitchToLoginQrCode => {
                self.page = AppPage::Login;
                self.login.emit(LoginInput::PairWithQrCode);
            }
            AppMsg::SwitchToLoginPhoneNumber => {
                self.page = AppPage::Login;
                self.login
                    .emit(LoginInput::PairWithPhoneNumber { edit: false });
            }

            AppMsg::ChatOpen => {
                self.split_view.set_show_content(true);
                self.session_page = AppSessionPage::ChatHistory;
            }
            AppMsg::ChatClosed => {
                self.chat_list.emit(ChatListInput::ClearSelection);
                self.split_view.set_show_content(false);
                self.session_page = AppSessionPage::Empty;
            }
            AppMsg::ChatSelected(jid) => {
                if let Some(chat) = self.chats.iter().find(|c| c.jid == jid).cloned() {
                    self.chat_view.emit(ChatViewInput::Open(chat));
                }

                if let Some(state) = self.typing.get(&jid)
                    && !state.senders.is_empty()
                {
                    self.emit_typing(&jid);
                }
            }
            AppMsg::MarkChatRead(jid) => {
                self.mark_chat_read(&jid).await;
            }

            AppMsg::AvatarUpdate { jid, path } => {
                // Update the chat's avatar path.
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    chat.avatar_path = Some(path);

                    // Update in chat list.
                    self.chat_list.emit(ChatListInput::UpdateChat {
                        chat: chat.clone(),
                        move_to_top: false,
                    });

                    tracing::debug!("Updated avatar for chat: {}", jid);
                }
            }
            AppMsg::ContactUpdate {
                jid,
                name,
                push_name,
                phone_number,
            } => {
                let display = name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .or_else(|| push_name.clone().filter(|n| !n.is_empty()));
                if let Some(display) = display {
                    self.contacts.insert(jid.clone(), display);
                }

                // Save contact to database in background.
                let db = self.db.clone();
                let jid_for_contact = jid.clone();
                let name_for_contact = name.clone();

                let contact = Contact {
                    jid: jid.clone(),
                    name: name.clone(),
                    push_name,
                    last_updated: 0,
                    phone_number: Some(phone_number),
                    is_registered: true,
                    profile_picture_url: None,
                };

                relm4::spawn(async move {
                    if let Err(e) = db.save_contact(&contact).await {
                        tracing::error!("Failed to save contact {}: {}", jid_for_contact, e);
                    } else {
                        tracing::debug!(
                            "Saved contact: {} (name: {:?})",
                            jid_for_contact,
                            name_for_contact
                        );
                    }
                });

                // Update chat name if this contact has a chat and we got a name.
                if let Some(contact_name) = name
                    && let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid)
                {
                    // Only update if current name is generic (phone number or "Unknown").
                    let current_name = chat.get_name_or_number();
                    let is_generic = current_name == contact_name
                        || current_name == i18n!("Unknown")
                        || current_name == format_lid_as_number(&jid);

                    if is_generic {
                        chat.name.clone_from(&contact_name);

                        // Save updated chat in background.
                        let jid_clone = jid.clone();
                        let chat_clone = chat.clone();

                        relm4::spawn(async move {
                            if let Err(e) = chat_clone.upsert().await {
                                tracing::error!(
                                    "Failed to update chat name for {}: {}",
                                    jid_clone,
                                    e
                                );
                            }
                        });

                        // Update in chat list immediately.
                        self.chat_list.emit(ChatListInput::UpdateChat {
                            chat: chat.clone(),
                            move_to_top: false,
                        });

                        tracing::info!("Updated chat name for {} to: {}", jid, contact_name);
                    }
                }
            }
            AppMsg::ReceiptUpdate {
                chat_jid,
                message_ids,
                receipt_type,
            } => {
                if let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned() {
                    match MessageStatus::try_from(receipt_type) {
                        Ok(status) => {
                            for msg_id in message_ids {
                                match chat.find_message(&msg_id).await {
                                    Ok(Some(mut message)) => {
                                        // Update message status.
                                        message.status = status;

                                        self.chat_view.emit(ChatViewInput::MessageStatusUpdate {
                                            status: message.status,
                                            local_id: message.local_id,
                                        });

                                        // Update the message in the database.
                                        let db = self.db.clone();
                                        let local_id = message.local_id;
                                        relm4::spawn(async move {
                                            if let Err(e) =
                                                db.set_message_status(local_id, status).await
                                            {
                                                tracing::error!(
                                                    "Failed to update message status: {}",
                                                    e
                                                );
                                            }
                                        });
                                    }
                                    Ok(None) => {
                                        tracing::warn!("Message {} not found", msg_id);
                                    }
                                    Err(e) => {
                                        tracing::warn!("Message {} not found: {e}", msg_id);
                                    }
                                }
                            }

                            self.chat_list.emit(ChatListInput::UpdateChat {
                                chat,
                                move_to_top: false,
                            });
                        }
                        Err(e) => tracing::error!(
                            "Failed to convert `ReceiptType` to `MessageStatus`: {e}"
                        ),
                    }
                }
            }
            AppMsg::PresenceUpdate {
                jid,
                available,
                last_seen,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    if !chat.is_group() {
                        chat.available = Some(available);
                    }

                    chat.last_seen = last_seen;
                }

                self.chat_view.emit(ChatViewInput::PresenceUpdate {
                    jid,
                    available,
                    last_seen,
                });
            }
            AppMsg::ChatPresenceUpdate {
                chat_jid,
                active,
                recording,
                sender_jid,
                sender_alt,
            } => {
                let name = self.chats.iter().find(|c| c.jid == chat_jid).map(|chat| {
                    let resolved = if chat.is_group() {
                        chat.participants
                            .get(&sender_jid)
                            .or_else(|| {
                                sender_alt
                                    .as_ref()
                                    .and_then(|alt| chat.participants.get(alt))
                            })
                            .filter(|name| !name.is_empty() && **name != i18n!("Unknown"))
                            .map_or_else(
                                || {
                                    let sender_jid = sender_jid.as_str();
                                    let contact = self
                                        .contacts
                                        .get(sender_jid)
                                        .or_else(|| {
                                            sender_alt
                                                .as_ref()
                                                .and_then(|alt| self.contacts.get(alt.as_str()))
                                        })
                                        .filter(|name| !name.is_empty());

                                    if let Some(contact) = contact {
                                        return contact.clone();
                                    }

                                    let phone = if sender_jid.ends_with("@lid") {
                                        sender_alt.as_deref().filter(|alt| !alt.ends_with("@lid"))
                                    } else {
                                        Some(sender_jid)
                                    };
                                    if phone.is_none() {
                                        self.client.emit(ClientInput::ResolveLidPn {
                                            chat_jid: chat_jid.clone(),
                                            lid: sender_jid.to_string(),
                                        });
                                    }

                                    phone.map_or_else(|| i18n!("Someone"), format_lid_as_number)
                                },
                                String::clone,
                            )
                    } else {
                        chat.name.trim().to_string()
                    };

                    if resolved.starts_with('+') {
                        resolved
                    } else {
                        get_first_name(&resolved)
                    }
                });

                if active && let Some(name) = name {
                    let state = self.typing.entry(chat_jid.clone()).or_default();
                    state.generation += 1;
                    let generation = state.generation;

                    state
                        .senders
                        .insert(sender_jid.clone(), TypingSender { name, recording });
                    self.emit_typing(&chat_jid);

                    let sender = sender.clone();
                    relm4::spawn(async move {
                        time::sleep(Duration::from_secs(10)).await;
                        sender.input(AppMsg::TypingExpired {
                            chat_jid,
                            sender_jid,
                            generation,
                        });
                    });
                } else {
                    let removed = self
                        .typing
                        .get_mut(&chat_jid)
                        .is_some_and(|state| state.senders.shift_remove(&sender_jid).is_some());
                    if removed {
                        self.emit_typing(&chat_jid);
                    }
                }
            }
            AppMsg::MessageStatusUpdate {
                chat_jid,
                msg_id,
                status,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
                    && let Ok(Some(mut message)) = chat.find_message_by_local_id(&msg_id).await
                {
                    message.status = status;

                    self.chat_view.emit(ChatViewInput::MessageStatusUpdate {
                        status: message.status,
                        local_id: message.local_id,
                    });

                    let db = self.db.clone();
                    relm4::spawn(async move {
                        if let Err(e) = db.set_message_status(msg_id, status).await {
                            tracing::error!("Failed to update message status: {}", e);
                        }
                    });
                }
            }

            AppMsg::LidPnResolved {
                chat_jid,
                lid,
                phone,
            } => {
                if let Some(phone) = phone
                    && chat_jid.ends_with("@g.us")
                {
                    let pn_jid = format!("{phone}@s.whatsapp.net");
                    let name = self
                        .contacts
                        .get(&pn_jid)
                        .cloned()
                        .filter(|n| !n.is_empty());

                    self.register_participant(&chat_jid, &lid, name.as_deref());
                    self.register_participant(&chat_jid, &pn_jid, name.as_deref());

                    let resolved = name.unwrap_or_else(|| format_lid_as_number(&pn_jid));
                    self.contacts.insert(lid.clone(), resolved.clone());

                    if let Some(state) = self.typing.get(&chat_jid)
                        && let Some(recording) = state.senders.get(&lid).map(|s| s.recording)
                    {
                        let display = if resolved.starts_with('+') {
                            resolved
                        } else {
                            get_first_name(&resolved)
                        };
                        if let Some(state) = self.typing.get_mut(&chat_jid) {
                            state.senders.insert(
                                lid,
                                TypingSender {
                                    name: display,
                                    recording,
                                },
                            );
                        }

                        self.emit_typing(&chat_jid);
                    }
                }
            }
            AppMsg::TypingExpired {
                chat_jid,
                sender_jid,
                generation,
            } => {
                let removed = self.typing.get_mut(&chat_jid).is_some_and(|state| {
                    state.generation == generation
                        && state.senders.shift_remove(&sender_jid).is_some()
                });
                if removed {
                    self.emit_typing(&chat_jid);
                }
            }
            AppMsg::TypingStateChanged {
                chat_jid,
                composing,
            } => {
                if composing {
                    self.client.emit(ClientInput::SendTyping { jid: chat_jid });
                } else {
                    self.client.emit(ClientInput::StopTyping { jid: chat_jid });
                }
            }

            AppMsg::MessageReceived { info, message } => {
                let content = message
                    .conversation
                    .clone()
                    .filter(|c| !c.is_empty())
                    .or_else(|| {
                        message
                            .extended_text_message
                            .as_option()
                            .and_then(|e| e.text.clone().filter(|t| !t.is_empty()))
                    });

                if let Some(content) = content {
                    if content == "status@broadcast" {
                        // TODO: handle status events
                    } else {
                        let chat_jid = info.source.chat.to_string();
                        let outgoing = info.source.is_from_me;

                        let status = MessageStatus::Sent;
                        let chat_message = ChatMessage {
                            local_id: Uuid::new_v4(),
                            server_id: info.id.clone(),
                            chat_jid: chat_jid.clone(),
                            sender_jid: if outgoing {
                                self.user_jid.clone().unwrap_or_default()
                            } else {
                                let sender = info.source.sender.to_string();
                                if sender.ends_with("@lid")
                                    && let Some(alt) = info.source.sender_alt.as_ref()
                                    && !alt.to_string().ends_with("@lid")
                                {
                                    alt.to_string()
                                } else {
                                    sender
                                }
                            },
                            sender_name: Some(info.push_name.clone()),

                            media: None,
                            status,
                            content,
                            outgoing,
                            reactions: IndexMap::new(),
                            timestamp: Timestamp::from_second(info.timestamp.timestamp())
                                .expect("Invalid timestamp"),

                            db: self.db.clone(),
                        };

                        self.add_message(&chat_jid, chat_message);
                        if !outgoing {
                            let sender_jid = info.source.sender.to_string();
                            let name =
                                (!info.push_name.is_empty()).then_some(info.push_name.as_str());
                            self.register_participant(&chat_jid, &sender_jid, name);

                            let alt_jid = info
                                .source
                                .sender_alt
                                .as_ref()
                                .map(std::string::ToString::to_string);
                            if let Some(alt_jid) = alt_jid.as_ref() {
                                self.register_participant(&chat_jid, alt_jid, name);
                            }

                            let removed = self.typing.get_mut(&chat_jid).is_some_and(|state| {
                                state.senders.shift_remove(&sender_jid).is_some()
                                    || alt_jid.as_ref().is_some_and(|alt| {
                                        state.senders.shift_remove(alt).is_some()
                                    })
                            });
                            if removed {
                                self.emit_typing(&chat_jid);
                            }
                        }
                    }
                } else if let Some(sent_message) = message.device_sent_message.as_option() {
                    if let Some(_chat_jid) = sent_message.destination_jid.as_ref() {
                        if let Some(msg) = sent_message.message.as_option() {
                            if let Some(_reaction) = msg.reaction_message.as_option() {
                                // TODO: handle
                            } else if let Some(_sticker) = msg.sticker_message.as_option() {
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

            AppMsg::SendTextMessage { text, recipient } => {
                // Get the chat if it exists and is loaded.
                if let Some(chat) = self.chats.iter().find(|c| c.jid == recipient).cloned() {
                    let timestamp = Timestamp::now();
                    let message = ChatMessage {
                        local_id: Uuid::new_v4(),
                        server_id: String::new(), // will be replaced later by the client.
                        chat_jid: recipient,
                        sender_jid: self.user_jid.clone().unwrap_or_default(),
                        sender_name: self.user_push_name.clone(),

                        media: None,
                        status: MessageStatus::Sending,
                        content: text,
                        outgoing: true,
                        reactions: IndexMap::new(),
                        timestamp,

                        db: self.db.clone(),
                    };

                    // Save the message in the database.
                    let msg_clone = message.clone();
                    relm4::spawn(async move {
                        if let Err(e) = msg_clone.upsert().await {
                            tracing::error!("Failed to save message: {}", e);
                        }
                    });

                    self.client.emit(ClientInput::SendMessage {
                        message: Box::new(message.clone()),
                    });
                    self.chat_view
                        .emit(ChatViewInput::MessageReceived(Box::new(message)));
                    self.chat_list.emit(ChatListInput::UpdateChat {
                        chat,
                        move_to_top: true,
                    });
                }
            }

            AppMsg::ChatsSynced { entries } => {
                sender.oneshot_command(async move { AppCmd::SyncChats { entries } });
            }

            AppMsg::ChatPropertyUpdate {
                jid,
                pinned,
                muted,
                archived,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    if let Some(pinned) = pinned {
                        chat.pinned = pinned;
                    }
                    if let Some(muted) = muted {
                        chat.muted = muted;
                    }
                    if let Some(archived) = archived {
                        chat.archived = archived;
                    }

                    // Always save the chat to the database (including archive state).
                    let chat_clone = chat.clone();
                    relm4::spawn(async move {
                        if let Err(e) = chat_clone.upsert().await {
                            tracing::error!("Failed to save chat property update: {}", e);
                        }
                    });

                    // Handle UI updates based on archive state.
                    if let Some(archived) = archived {
                        if archived {
                            // Remove from chat list UI.
                            self.chat_list
                                .emit(ChatListInput::RemoveChat { jid: jid.clone() });
                        } else {
                            // Unarchive: add back to chat list (AddChat handles both
                            // new and existing entries).
                            self.chat_list.emit(ChatListInput::AddChat {
                                chat: chat.clone(),
                                at_top: false,
                            });
                        }
                    } else {
                        // Pin/mute only — update in place.
                        self.chat_list.emit(ChatListInput::UpdateChat {
                            chat: chat.clone(),
                            move_to_top: false,
                        });
                    }
                }
            }
            AppMsg::HistorySyncCompleted => {
                tracing::info!("History sync completed");
                if self.state == AppState::Syncing {
                    self.state = AppState::Ready;
                }
            }
            AppMsg::OfflineSyncCompleted => {
                tracing::info!("Offline sync completed");
                if self.state == AppState::Syncing {
                    self.state = AppState::Ready;
                }
            }

            AppMsg::Unknown => {}
            AppMsg::Error { message } => {
                self.state = AppState::Error(message.clone());

                match self.page {
                    AppPage::Login => {
                        self.login.emit(LoginInput::Error { message });
                    }
                    AppPage::Session | AppPage::Welcome | AppPage::Error => {
                        // TODO: display error
                    }
                }
            }
            AppMsg::Quit => main_application().quit(),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            AppCmd::LoadCache => {
                self.state = AppState::Syncing;
                let mut chats_needing_avatars = Vec::new();

                match self.db.load_chats().await {
                    Ok(mut chats) => {
                        tracing::info!("Loaded {} chats from own database", chats.len());

                        // Check for existing cached avatars.
                        for chat in &mut chats {
                            if let Some(name) =
                                self.contacts.get(&chat.jid).filter(|n| !n.is_empty())
                            {
                                chat.name.clone_from(name);
                            }

                            // Check if avatar exists in cache.
                            let avatar_path = DATA_DIR.join("avatars").join(format!(
                                "{}.jpg",
                                chat.jid
                                    .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
                            ));
                            if avatar_path.exists() {
                                chat.avatar_path = Some(avatar_path.to_string_lossy().into_owned());
                            } else {
                                chats_needing_avatars.push(chat.jid.clone());
                            }
                        }

                        // Insert all chats into our cached list.
                        self.chats.extend(chats);

                        for chat in &self.chats {
                            // Add the chat to the chat list.
                            self.chat_list.emit(ChatListInput::AddChat {
                                chat: chat.clone(),
                                at_top: false,
                            });
                        }
                    }
                    Err(e) => tracing::error!("Failed to load chats from own database: {}", e),
                }

                self.state = AppState::Ready;

                // Emit `SyncCompleted` to fetch avatars in the regular update cycle.
                if !chats_needing_avatars.is_empty() {
                    sender.input(AppMsg::SyncCompleted {
                        chats_needing_avatars,
                    });
                }
            }
            AppCmd::SwitchSession { db, session } => {
                self.db = db;
                self.session = session;
            }

            AppCmd::SyncChats { entries } => {
                for entry in entries {
                    let ChatsSyncedEntry {
                        jid,
                        name,
                        pinned,
                        archived,
                        messages,
                        participants,
                        last_message_time,
                        ..
                    } = entry;
                    let chat_jid = jid.clone();

                    let is_new_chat = !self.chats.iter().any(|c| c.jid == jid);
                    if is_new_chat {
                        self.client.emit(ClientInput::FetchAvatar {
                            jid: chat_jid.clone(),
                        });

                        // Determine chat name.
                        let chat_name = name.unwrap_or_else(|| {
                            if jid.ends_with("@g.us") {
                                format!("{} {}", i18n!("Group"), &jid[..8.min(jid.len())])
                            } else if self.user_jid.as_ref().is_some_and(|u_j| jid == *u_j) {
                                i18n!("You")
                            } else {
                                format_lid_as_number(&jid)
                            }
                        });

                        // Create last message time from timestamp (already in seconds).
                        let last_message_time = last_message_time
                            .and_then(|ts| Timestamp::from_second(ts.cast_signed()).ok())
                            .unwrap_or_else(Timestamp::now);

                        // Create participants map for groups.
                        let mut participants_map = HashMap::new();
                        for (pjid, pname) in participants {
                            participants_map
                                .insert(pjid, pname.unwrap_or_else(|| i18n!("Unknown")));
                        }

                        let chat = Chat {
                            jid,
                            name: chat_name,
                            muted: false, // TODO: handle mute_end_time
                            pinned,
                            archived,
                            available: None,
                            last_seen: None,
                            avatar_path: None,
                            participants: participants_map,
                            last_message_time,

                            db: self.db.clone(),
                        };

                        // Add to cached list (keep in memory for property updates even if archived).
                        self.chats.push(chat.clone());

                        // Sort chats.
                        self.chats.sort_by(|a, b| {
                            b.pinned
                                .cmp(&a.pinned)
                                .then_with(|| b.last_message_time.cmp(&a.last_message_time))
                        });

                        // Save the chat to database in blocking thread (fire and forget).
                        relm4::spawn(async move {
                            if let Err(e) = chat.upsert().await {
                                tracing::error!("Failed to save synced chat {}: {}", chat.jid, e);
                            } else {
                                tracing::debug!(
                                    "Synced chat from history: {} (archived: {}, pinned: {})",
                                    chat.jid,
                                    archived,
                                    pinned
                                );
                            }
                        });
                    }

                    if messages.is_empty() {
                        continue;
                    }

                    let is_group = chat_jid.ends_with("@g.us");

                    // Update chat in the list (lightweight UI update) before moving values.
                    if let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned() {
                        self.chat_list.emit(ChatListInput::UpdateChat {
                            chat,
                            move_to_top: false,
                        });
                    }

                    let db = self.db.clone();

                    // Collect sender info for participant updates.
                    let sender_info: Vec<(String, Option<String>)> = if is_group {
                        messages
                            .iter()
                            .filter(|m| !m.outgoing && !m.sender_jid.is_empty())
                            .map(|m| (m.sender_jid.clone(), m.sender_name.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    };

                    // Update participants for groups immediately (in-memory).
                    if is_group
                        && let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
                    {
                        for (sender_jid, sender_name) in &sender_info {
                            let entry = chat
                                .participants
                                .entry(sender_jid.clone())
                                .or_insert_with(|| i18n!("Unknown"));
                            if let Some(name) = sender_name.as_deref().filter(|n| !n.is_empty())
                                && *entry == i18n!("Unknown")
                            {
                                *entry = name.to_string();
                            }
                        }
                    }

                    let chat_clone = self.chats.iter().find(|c| c.jid == chat_jid).cloned();
                    let sender = self.sender.clone();

                    // Spawn database operations in background task.
                    relm4::spawn(async move {
                        let mut saved_count = 0;
                        let mut dup_count = 0;
                        let mut skip_count = 0;
                        let total = messages.len();

                        for synced_msg in messages {
                            // Skip messages without content for now.
                            let Some(content) = synced_msg.content else {
                                skip_count += 1;
                                continue;
                            };

                            // Select message status based on `unread` and `outgoing` fields.
                            let status = match (synced_msg.unread, synced_msg.outgoing) {
                                (true, false) => MessageStatus::Delivered,
                                (true, true) => MessageStatus::Sent,
                                (false, _) => MessageStatus::Read,
                            };

                            // Timestamp is already in seconds (Unix timestamp).
                            let timestamp =
                                Timestamp::from_second(synced_msg.timestamp.cast_signed())
                                    .unwrap_or_else(|_| Timestamp::now());

                            let message = ChatMessage {
                                local_id: Uuid::new_v4(),
                                server_id: synced_msg.id,
                                chat_jid: chat_jid.clone(),
                                sender_jid: synced_msg.sender_jid.clone(),
                                sender_name: synced_msg.sender_name.clone(),

                                media: None,
                                status,
                                content,
                                outgoing: synced_msg.outgoing,
                                reactions: IndexMap::new(),
                                timestamp,

                                db: db.clone(),
                            };

                            // Save the message, skipping duplicates on server_id.
                            match message.save_or_ignore().await {
                                Ok(true) => saved_count += 1,
                                Ok(false) => dup_count += 1,
                                Err(e) => tracing::error!("Failed to save synced message: {}", e),
                            }
                        }

                        tracing::debug!(
                            "Synced {} messages for chat: {} (of {} received, {} duplicates, {} without content)",
                            saved_count,
                            chat_jid,
                            total,
                            dup_count,
                            skip_count
                        );

                        if let Some(chat) = chat_clone {
                            if is_new_chat && !archived {
                                sender
                                    .oneshot_command(async move { AppCmd::AddChatToList { chat } });
                            } else {
                                sender.oneshot_command(async move {
                                    AppCmd::UpdateChat {
                                        chat,
                                        move_to_top: false,
                                    }
                                });
                            }
                        }
                    });
                }
            }

            AppCmd::UpdateChat { chat, move_to_top } => {
                self.chat_list
                    .emit(ChatListInput::UpdateChat { chat, move_to_top });
            }
            AppCmd::AddChatToList { chat } => {
                self.chat_list
                    .emit(ChatListInput::AddChat { chat, at_top: true });
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
