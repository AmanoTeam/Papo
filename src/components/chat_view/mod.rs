mod history;
mod rows;

use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use jiff::{Timestamp, Zoned, tz::TimeZone};
use relm4::prelude::*;
use tokio::time;
use uuid::Uuid;

use self::{history::ChatHistory, rows::ChatRow};
use crate::{
    db::store::SessionStore,
    i18n, i18n_f,
    state::{Chat, ChatMessage, HistoryAnchor, MessageStatus, TypingSender},
    utils::{bare_jid, is_status_broadcast, re_resolve_senders},
    widgets::TypingDots,
};

/// Rows emitted per step while streaming history in.
const LOAD_MORE_COUNT: usize = 20;
/// First slice of the inital window shown at once.
const FILL_STREAM_CHUNK: usize = 20;
const INITIAL_LOAD_COUNT: usize = 40;
/// Cooldown between on-demand history requests for one chat.
const HISTORY_FETCH_RETRY: Duration = Duration::from_secs(60);
/// How long to wait for a phone answer before asking again. Requests
/// made while the phone app is asleep are dropped rather than queued.
const HISTORY_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Read-message rows kept visible above the unread divider at open.
const UNREAD_BAND_CONTEXT_ROWS: u32 = 4;

#[derive(Debug)]
pub struct ChatView {
    db: SessionStore,
    chat: Option<Chat>,
    state: ChatViewState,
    history: ChatHistory,
    /// Monotonic generation counter, incremented on every chat open or jump
    /// reload. Used to discard stale command results from a previous chat.
    generation: u64,
    message_entry: gtk::Entry,
    typing_avatars: gtk::Box,
    /// In-flight marker for an on-demand history request; doubles as the
    /// retry clock, since a response or the retry window clears it.
    history_fetch_at: Option<Instant>,
    history_exhausted: bool,
    history_fetch_anchored: bool,
    /// Sender avatars already requested this session; group rows ask
    /// per bind, so the set keeps repeat binds off the wire.
    sender_avatar_requests: HashSet<String>,
}

/// Feedback for an ongoing history sync, shown in the banner.
#[derive(Debug)]
struct SyncFeedback {
    total: usize,
    synced: usize,
    percent: Option<u32>,
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChatViewState {
    sync: Option<SyncFeedback>,
    typing: Vec<TypingSender>,
    presence: Option<String>,
    is_typing: bool,
    is_loading: bool,
    is_at_top: bool,
    is_at_bottom: bool,
    unread_count: usize,
    typing_generation: u64,
}

impl ChatViewState {
    /// The state a freshly opened chat view starts from.
    fn new() -> Self {
        Self {
            sync: None,
            typing: Vec::new(),
            presence: None,
            is_typing: false,
            is_loading: true,
            is_at_top: false,
            is_at_bottom: true,
            unread_count: 0,
            typing_generation: 0,
        }
    }
}

#[derive(Debug)]
pub enum ChatViewInput {
    Open(Chat),
    Close,

    SendMessage,
    EntryChanged,
    MessageReceived(Box<ChatMessage>),

    TypingUpdate {
        chat_jid: String,
        senders: Vec<TypingSender>,
    },
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<Timestamp>,
    },
    SenderAvatarUpdated {
        jid: String,
    },
    MessageStatusUpdate {
        status: MessageStatus,
        local_id: Uuid,
    },

    ChatUpdated {
        chat: Chat,
    },
    SenderNameChanged {
        forms: Vec<String>,
        name: Option<String>,
    },

    SenderAvatarRetry {
        jid: String,
    },
    RequestSenderAvatar {
        jid: String,
    },

    SyncProgress {
        active: bool,
        percent: Option<u32>,
        synced: usize,
        total: usize,
    },
    HistoryBackfilled {
        chat_jid: String,
        has_messages: bool,
    },

    ScrollToBottom,
}

#[derive(Debug)]
pub enum ChatViewOutput {
    ChatOpen,
    ChatClosed,
    MarkChatRead(String),

    SendTextMessage {
        text: String,
        recipient: String,
    },

    TypingStateChanged {
        chat_jid: String,
        composing: bool,
    },

    FetchHistory {
        chat_jid: String,
        anchor: Option<HistoryAnchor>,
    },

    RequestSenderAvatar {
        jid: String,
    },
}

#[derive(Debug)]
pub enum ChatViewCommand {
    FillChunk {
        generation: u64,
        messages: Vec<ChatMessage>,
        /// Digit-keyed LID identity map from the resolving pass, for
        /// the timeline's author grouping.
        identities: HashMap<String, String>,
    },
    InitialMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        identities: HashMap<String, String>,
        had_unread: bool,
    },
    OlderMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        identities: HashMap<String, String>,
    },
    NewerMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        identities: HashMap<String, String>,
    },

    JumpLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        identities: HashMap<String, String>,
    },
    FetchRetryTimeout {
        generation: u64,
    },
    BackfillLoaded {
        generation: u64,
        older: Vec<ChatMessage>,
        newer: Vec<ChatMessage>,
        identities: HashMap<String, String>,
    },

    ScrollPositionChanged {
        at_top: bool,
        at_bottom: bool,
    },

    TypingTimeout {
        generation: u64,
    },
}

#[relm4::component(async, pub)]
impl AsyncComponent for ChatView {
    type Init = SessionStore;
    type Input = ChatViewInput;
    type Output = ChatViewOutput;
    type CommandOutput = ChatViewCommand;

    view! {
        adw::ToolbarView {
            set_css_classes: &["chat-view"],
            set_bottom_bar_style: adw::ToolbarStyle::Flat,

            add_top_bar = &adw::HeaderBar {
                set_css_classes: &["flat"],

                #[wrap(Some)]
                set_title_widget = &gtk::Button {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    #[watch]
                    set_css_classes: &["chat-title", "flat", if model.state.presence.is_some() { "with-subtitle" } else { "" }],

                    gtk::Box {
                        set_halign: gtk::Align::Center,
                        set_valign: gtk::Align::Center,
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Label {
                            set_wrap: false,
                            #[watch]
                            set_label?: model.chat.as_ref().map(Chat::get_name_or_number).as_ref(),
                            #[watch]
                            set_visible: model.chat.is_some(),
                            set_ellipsize: pango::EllipsizeMode::End,
                            set_selectable: false,
                            set_width_chars: 5,
                            set_css_classes: &["title"],
                            #[watch]
                            set_tooltip_text?: model
                                .chat
                                .as_ref()
                                .map(|chat| Some(chat.name.as_str())),
                            set_single_line_mode: true,
                        },

                        gtk::Label {
                            #[watch]
                            set_label?: model.state.presence.as_ref(),
                            #[watch]
                            set_visible: model.state.presence.is_some(),
                            set_selectable: false,
                            set_ellipsize: pango::EllipsizeMode::End,
                            set_max_width_chars: 40,
                            set_css_classes: &["subtitle"],
                        },
                    },
                },
            },

            add_top_bar = &gtk::Revealer {
                #[watch]
                set_reveal_child: model.state.sync.is_some(),
                set_transition_type: gtk::RevealerTransitionType::SlideDown,
                set_transition_duration: 250,

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_css_classes: &["sync-banner"],

                    gtk::Box {
                        set_spacing: 6,
                        set_orientation: gtk::Orientation::Horizontal,

                        set_margin_top: 6,
                        set_margin_bottom: 6,
                        set_margin_start: 12,
                        set_margin_end: 12,

                        adw::Spinner {
                            set_width_request: 16,
                            set_height_request: 16,
                        },

                        gtk::Label {
                            #[watch]
                            set_label: model.sync_banner_label().as_str(),
                            set_halign: gtk::Align::Start,
                            set_hexpand: true,
                            set_ellipsize: pango::EllipsizeMode::End,
                            set_css_classes: &["dimmed"],
                        },
                    },

                    gtk::ProgressBar {
                        #[watch]
                        set_visible: model
                            .state
                            .sync
                            .as_ref()
                            .is_some_and(|sync| sync.percent.is_some()),
                        set_hexpand: true,
                        #[watch]
                        set_fraction?: model.banner_sync_fraction(),
                    },
                },
            },

            #[wrap(Some)]
            set_content = &gtk::Overlay {
                #[wrap(Some)]
                #[local_ref]
                set_child = &scroll_window -> gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    set_overlay_scrolling: true,

                    adw::ClampScrollable {
                        set_maximum_size: 960,
                        set_vscroll_policy: gtk::ScrollablePolicy::Natural,
                        set_tightening_threshold: 400,

                        #[local_ref]
                        list_view -> gtk::ListView {
                            set_css_classes: &["chat-history"]
                        }
                    },
                },

                add_overlay = &gtk::Revealer {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Start,
                    #[watch]
                    set_reveal_child: model.chat.is_some()
                        && (model.state.is_loading || model.is_fetching_history()),
                    set_transition_type: gtk::RevealerTransitionType::Crossfade,
                    set_transition_duration: 350,

                    gtk::Box {
                        set_spacing: 8,
                        set_margin_top: 12,
                        set_css_classes: &["service-message", "loading-toast"],
                        set_orientation: gtk::Orientation::Horizontal,

                        adw::Spinner {
                            set_width_request: 3,
                            set_height_request: 3
                        },

                        gtk::Label {
                            set_label: &i18n!("Loading messages..."),
                            set_css_classes: &["caption"]
                        }
                    }
                },

                add_overlay = &gtk::Revealer {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::End,
                    #[watch]
                    set_reveal_child: model.chat.is_some() && !model.state.is_at_bottom,
                    set_transition_type: gtk::RevealerTransitionType::Crossfade,
                    set_transition_duration: 350,

                    gtk::Overlay {
                        gtk::Button {
                            set_icon_name: "down-small-symbolic",
                            set_css_classes: &["circular", "osd"],
                            set_margin_bottom: 12,

                            connect_clicked => ChatViewInput::ScrollToBottom
                        },

                        add_overlay = &gtk::Label {
                            #[watch]
                            set_label: model.unread_badge_label().as_str(),
                            set_halign: gtk::Align::End,
                            set_valign: gtk::Align::Start,
                            #[watch]
                            set_visible: model.state.unread_count > 0,
                            set_css_classes: &["badge"],
                        },
                    },
                },

            },

            add_bottom_bar = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                gtk::Revealer {
                    #[watch]
                    set_reveal_child: model.chat.is_some() && !model.state.typing.is_empty(),
                    set_transition_type: gtk::RevealerTransitionType::SlideUp,
                    set_transition_duration: 250,

                    gtk::Box {
                        set_spacing: 6,
                        set_margin_start: 6,
                        set_margin_top: 6,
                        set_margin_end: 6,

                        #[local_ref]
                        typing_avatars -> gtk::Box {
                            set_css_classes: &["typing-avatars"]
                        },

                        gtk::Box {
                            set_valign: gtk::Align::Center,

                            TypingDots {},
                        },

                        gtk::Label {
                            #[watch]
                            set_label: model.typing_text().as_str(),
                            set_halign: gtk::Align::Start,
                            set_valign: gtk::Align::Center,
                            set_css_classes: &["dimmed"],
                            set_ellipsize: pango::EllipsizeMode::End,
                        },
                    },
                },

                gtk::Box {
                    set_spacing: 6,
                    set_margin_all: 6,
                    set_orientation: gtk::Orientation::Horizontal,

                    #[local_ref]
                    message_entry -> gtk::Entry {
                        set_hexpand: true,
                        set_placeholder_text: Some(&i18n!("Type a message...")),

                        connect_activate => ChatViewInput::SendMessage,
                        connect_changed => ChatViewInput::EntryChanged,
                    },

                    gtk::Button {
                        set_icon_name: "paper-plane-symbolic",
                        set_css_classes: &["circular", "suggested-action"],

                        connect_clicked => ChatViewInput::SendMessage,
                    },
                },
            },
        }
    }

    #[allow(clippy::unused_async_trait_impl)]
    async fn init(
        db: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let history = ChatHistory::new(Some(sender.input_sender().clone()));
        let scroll_window = gtk::ScrolledWindow::new();

        let model = Self {
            db,
            chat: None,
            state: ChatViewState::new(),
            history,
            generation: 0,
            message_entry: gtk::Entry::new(),
            typing_avatars: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            history_fetch_at: None,
            history_exhausted: false,
            history_fetch_anchored: false,
            sender_avatar_requests: HashSet::new(),
        };

        let list_view = model.history.view().clone();
        let message_entry = &model.message_entry;
        let typing_avatars = &model.typing_avatars;
        let widgets = view_output!();

        // Focus the scroll window when clicked within.
        let scroll = scroll_window.clone();
        let click_gesture = gtk::GestureClick::new();
        click_gesture.connect_pressed(move |_, _, _, _| {
            scroll.grab_focus();
        });
        scroll_window.add_controller(click_gesture);

        // Return focus to message entry when `Esc` is pressed and scroll window is focused.
        let entry = message_entry.clone();
        let key_event_controller = gtk::EventControllerKey::new();
        key_event_controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                entry.grab_focus();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        scroll_window.add_controller(key_event_controller);

        // Close the chat when `Esc` is pressed and message entry is focused.
        let input_sender = sender.input_sender().clone();
        let key_event_controller = gtk::EventControllerKey::new();
        key_event_controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                input_sender.emit(ChatViewInput::Close);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        message_entry.add_controller(key_event_controller);

        // Track scroll position and notify the model when it changes.
        let adj = widgets.scroll_window.vadjustment();
        let command_sender = sender.command_sender().clone();

        // Scrolling past the top edge is a request for more history.
        let edge_sender = command_sender.clone();
        widgets
            .scroll_window
            .connect_edge_overshot(move |_, position| {
                if position == gtk::PositionType::Top {
                    edge_sender.emit(ChatViewCommand::ScrollPositionChanged {
                        at_top: true,
                        at_bottom: false,
                    });
                }
            });

        // Level-checked near the top, but only while the user is
        // actually scrolling up: an anchoring bump after a prepend
        // grows the value the other way, and without this filter those
        // bumps load page after page in a self-sustaining storm. NaN
        // seeds the previous value so the first tick never computes a
        // direction.
        let was_at_top = Rc::new(Cell::new(false));
        let was_at_bottom = Rc::new(Cell::new(true));
        let prev_value = Rc::new(Cell::new(f64::NAN));
        adj.connect_value_changed(move |adj| {
            let value = adj.value();
            let page = adj.page_size();
            let upper = adj.upper();
            let moving_up = value < prev_value.replace(value);

            let near_top = value < page * 2.0 && upper > page;
            let at_bottom = value + page >= upper - 25.0;

            if near_top && moving_up {
                was_at_top.set(true);
                was_at_bottom.set(false);

                command_sender.emit(ChatViewCommand::ScrollPositionChanged {
                    at_top: true,
                    at_bottom: false,
                });
            } else if !moving_up {
                let left_top = was_at_top.replace(false);
                let bottom_toggled = at_bottom != was_at_bottom.get();

                if left_top || bottom_toggled {
                    was_at_bottom.set(at_bottom);
                    command_sender.emit(ChatViewCommand::ScrollPositionChanged {
                        at_top: false,
                        at_bottom,
                    });
                }
            }
        });

        AsyncComponentParts { model, widgets }
    }

    #[allow(clippy::unused_async_trait_impl, clippy::too_many_lines)]
    async fn update(
        &mut self,
        input: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            ChatViewInput::Open(chat) => {
                if let Some(ref open) = self.chat
                    && open.jid == chat.jid
                {
                    return;
                }

                self.generation += 1;
                self.history.reset();
                self.history.set_group(chat.is_group());

                if self.state.is_typing
                    && let Some(ref old) = self.chat
                    && old.jid == chat.jid
                {
                    self.state.is_typing = false;
                    let _ = sender.output(ChatViewOutput::TypingStateChanged {
                        chat_jid: old.jid.clone(),
                        composing: false,
                    });
                }

                // Reset state.
                self.state.presence = None;
                self.state.is_loading = true;
                self.state.is_at_top = false;
                self.state.is_at_bottom = true;
                self.state.unread_count = 0;
                self.state.typing_generation += 1;

                self.history_fetch_at = None;
                self.history_exhausted = false;
                self.history_fetch_anchored = false;

                self.chat = Some(chat.clone());
                self.state.typing.clear();
                self.rebuild_typing_avatars();
                tracing::debug!("Opening chat: {}", chat.jid);

                // Update the user presence label.
                self.update_presence();

                // Grab message entry focus as convenience.
                self.message_entry.grab_focus();

                // Load the initial batch of messages.
                let db = self.db.clone();
                let generation = self.generation;
                sender.oneshot_command(async move {
                    let mut messages = chat
                        .load_messages(&db, INITIAL_LOAD_COUNT)
                        .await
                        .unwrap_or_default();
                    let identities = re_resolve_senders(&db, &chat, &mut messages).await;
                    let had_unread = chat
                        .get_unread_count(&db)
                        .await
                        .is_ok_and(|count| count > 0);
                    ChatViewCommand::InitialMessagesLoaded {
                        generation,
                        messages,
                        identities,
                        had_unread,
                    }
                });

                let _ = sender.output(ChatViewOutput::ChatOpen);
            }
            ChatViewInput::Close => {
                self.generation += 1;
                self.history.reset();

                if self.state.is_typing
                    && let Some(ref chat) = self.chat
                {
                    self.state.is_typing = false;
                    let _ = sender.output(ChatViewOutput::TypingStateChanged {
                        chat_jid: chat.jid.clone(),
                        composing: false,
                    });
                }

                // Reset state.
                self.chat = None;
                self.state.presence = None;
                self.state.is_loading = false;
                self.state.is_at_bottom = false;
                self.state.typing_generation += 1;
                self.state.typing.clear();
                self.rebuild_typing_avatars();

                let _ = sender.output(ChatViewOutput::ChatClosed);
            }

            ChatViewInput::SendMessage => {
                if let Some(ref chat) = self.chat
                    && self.message_entry.text_length() > 0
                {
                    let text = self.message_entry.text().to_string();
                    self.message_entry.set_text("");

                    // Send a plain text message.
                    let _ = sender.output(ChatViewOutput::SendTextMessage {
                        text,
                        recipient: chat.jid.clone(),
                    });

                    // TODO: implements media sending

                    if self.state.is_at_bottom {
                        let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                    }
                }
            }
            ChatViewInput::EntryChanged => {
                let Some(chat_jid) = self.chat.as_ref().map(|chat| chat.jid.clone()) else {
                    return;
                };

                if self.message_entry.text_length() > 0 {
                    self.state.typing_generation += 1;
                    let generation = self.state.typing_generation;

                    sender.oneshot_command(async move {
                        time::sleep(Duration::from_secs(5)).await;
                        ChatViewCommand::TypingTimeout { generation }
                    });

                    if !self.state.is_typing {
                        self.state.is_typing = true;

                        let _ = sender.output(ChatViewOutput::TypingStateChanged {
                            chat_jid,
                            composing: true,
                        });
                    }
                } else if self.state.is_typing {
                    self.state.is_typing = false;
                    self.state.typing_generation += 1;

                    let _ = sender.output(ChatViewOutput::TypingStateChanged {
                        chat_jid,
                        composing: false,
                    });
                }
            }
            ChatViewInput::MessageReceived(message) => {
                if self
                    .chat
                    .as_ref()
                    .is_none_or(|chat| chat.jid != message.chat_jid)
                {
                    return;
                }

                // If the bottom has been trimmed, skip appending — the message will
                // appear when the user scrolls back to bottom and triggers a reload.
                if self.history.has_newer() {
                    return;
                }

                let outgoing = message.outgoing;
                self.history.append_live(*message, self.state.is_at_bottom);

                if self.state.is_at_bottom {
                    self.scroll_to_bottom(|| {});
                } else if !outgoing {
                    self.state.unread_count += 1;
                }

                // If the user is at the bottom, they're seeing this message — mark read.
                if self.state.is_at_bottom
                    && let Some(ref chat) = self.chat
                {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                }
            }

            ChatViewInput::TypingUpdate { chat_jid, senders } => {
                if self.chat.as_ref().is_none_or(|chat| chat.jid != chat_jid) {
                    return;
                }

                let was_typing = !self.state.typing.is_empty();
                self.state.typing = senders;
                self.rebuild_typing_avatars();

                // The indicator expands below the timeline: when it first
                // appears, keep the bottom-most message fully visible for
                // readers parked at the bottom.
                if !was_typing && !self.state.typing.is_empty() && self.state.is_at_bottom {
                    self.scroll_to_bottom(|| {});
                }
            }
            ChatViewInput::PresenceUpdate {
                jid,
                available,
                last_seen,
            } => {
                if let Some(ref mut chat) = self.chat
                    && jid == chat.jid
                {
                    if !chat.is_group() {
                        chat.available = Some(available);
                    }
                    chat.last_seen = last_seen;

                    // Update the user presence label.
                    self.update_presence();
                }
            }
            ChatViewInput::SenderAvatarUpdated { jid } => {
                self.history.update_sender_avatars(&jid);
                self.rebuild_typing_avatars();
            }
            ChatViewInput::MessageStatusUpdate { local_id, status } => {
                if let Some(index) = self
                    .history
                    .find_message_index(|message| message.local_id == local_id)
                    && let Some(mut row) = self.history.get_row(index)
                {
                    if let ChatRow::Message { message, .. } = &mut row {
                        message.status = status;
                    }

                    self.history
                        .replace_row(index, row, self.state.is_at_bottom);
                }
            }

            ChatViewInput::ChatUpdated { chat } => {
                if let Some(current) = self.chat.as_mut()
                    && current.jid == chat.jid
                {
                    *current = chat;
                }
            }
            ChatViewInput::SenderNameChanged { forms, name } => {
                self.history.update_sender_names(&forms, name.as_deref());
            }

            ChatViewInput::SenderAvatarRetry { jid } => {
                self.sender_avatar_requests.remove(&bare_jid(&jid));
            }
            ChatViewInput::RequestSenderAvatar { jid } => {
                let bare = bare_jid(&jid);
                if self.sender_avatar_requests.insert(bare.clone()) {
                    let _ = sender.output(ChatViewOutput::RequestSenderAvatar { jid: bare });
                }
            }

            ChatViewInput::SyncProgress {
                active,
                percent,
                synced,
                total,
            } => {
                self.state.sync = active.then_some(SyncFeedback {
                    total,
                    synced,
                    percent,
                });
            }
            ChatViewInput::HistoryBackfilled {
                chat_jid,
                has_messages,
            } => {
                let Some(chat) = self.chat.clone() else {
                    return;
                };
                if chat.jid != chat_jid {
                    return;
                }

                self.history_fetch_at = None;

                // The phone answered with nothing for this chat: stop asking.
                if !has_messages {
                    self.history_exhausted = true;
                    return;
                }

                if self.history.is_empty() {
                    // The chat was empty when opened: reload the initial
                    // window now that history arrived.
                    self.state.is_loading = true;

                    let db = self.db.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let mut messages = chat
                            .load_messages(&db, INITIAL_LOAD_COUNT)
                            .await
                            .unwrap_or_default();
                        let identities = re_resolve_senders(&db, &chat, &mut messages).await;

                        ChatViewCommand::InitialMessagesLoaded {
                            generation,
                            messages,
                            identities,
                            had_unread: false,
                        }
                    });
                } else {
                    self.state.is_loading = true;

                    let before_ts = self.history.oldest_timestamp();
                    let after_ts = self.history.newest_timestamp();
                    let db = self.db.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let mut older = match before_ts {
                            Some(before_ts) => chat
                                .load_messages_before(&db, before_ts, LOAD_MORE_COUNT)
                                .await
                                .unwrap_or_default(),
                            None => Vec::new(),
                        };
                        let mut newer = match after_ts {
                            Some(after_ts) => chat
                                .load_messages_after(&db, after_ts, LOAD_MORE_COUNT)
                                .await
                                .unwrap_or_default(),
                            None => Vec::new(),
                        };
                        let identities = re_resolve_senders(&db, &chat, &mut older).await;
                        re_resolve_senders(&db, &chat, &mut newer).await;

                        ChatViewCommand::BackfillLoaded {
                            generation,
                            older,
                            newer,
                            identities,
                        }
                    });
                }
            }

            ChatViewInput::ScrollToBottom => {
                // If either end has been trimmed, the view is a "window" into the
                // message history — reload from scratch to jump to the real latest.
                if self.history.has_newer() || self.history.has_older() {
                    self.generation += 1;
                    self.history.reset();
                    self.state.is_loading = true;

                    if let Some(ref chat) = self.chat {
                        let db = self.db.clone();
                        let chat = chat.clone();
                        let generation = self.generation;
                        sender.oneshot_command(async move {
                            let mut messages = chat
                                .load_messages(&db, INITIAL_LOAD_COUNT)
                                .await
                                .unwrap_or_default();
                            let identities = re_resolve_senders(&db, &chat, &mut messages).await;

                            ChatViewCommand::JumpLoaded {
                                generation,
                                messages,
                                identities,
                            }
                        });
                    }
                } else {
                    // Scroll to the last message.
                    self.scroll_to_bottom(|| {});
                    self.state.is_at_bottom = true;
                    self.state.unread_count = 0;
                }
            }
        }
    }

    #[allow(clippy::unused_async_trait_impl, clippy::too_many_lines)]
    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            ChatViewCommand::InitialMessagesLoaded {
                generation,
                mut messages,
                identities,
                had_unread,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.set_identities(identities.clone());

                let full_len = messages.len();
                let tail = if !had_unread && full_len > FILL_STREAM_CHUNK {
                    messages.split_off(FILL_STREAM_CHUNK)
                } else {
                    Vec::new()
                };

                self.history.fill(&messages);
                self.history.set_has_older(full_len == INITIAL_LOAD_COUNT);

                self.state.is_loading = false;

                // A chat whose stored history is shorter than a page may be
                // missing older messages, including chats too short to
                // scroll: ask the phone for the history before the oldest
                // stored message.
                if full_len < INITIAL_LOAD_COUNT
                    && let Some(ref chat) = self.chat
                    && !is_status_broadcast(&chat.jid)
                {
                    let anchor = self.oldest_message_anchor();
                    self.history_fetch_at = Some(Instant::now());
                    self.history_fetch_anchored = anchor.is_some();

                    let _ = sender.output(ChatViewOutput::FetchHistory {
                        chat_jid: chat.jid.clone(),
                        anchor,
                    });
                    self.schedule_fetch_timeout(&sender);
                }

                if had_unread && let Some(jid) = self.chat.as_ref().map(|chat| chat.jid.clone()) {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(jid));
                }

                // A chat with unread messages opens at the first unread
                // one, with a bit of read context above the divider.
                // Everything else opens where the conversation is picked
                // up: the bottom.
                if let Some(divider) = self
                    .history
                    .unread_divider_index()
                    .filter(|divider| divider > &UNREAD_BAND_CONTEXT_ROWS)
                {
                    self.history
                        .scroll_to_unread_boundary(divider - UNREAD_BAND_CONTEXT_ROWS);
                    self.state.is_at_bottom = false;
                } else {
                    self.scroll_to_bottom(|| {});
                    self.state.is_at_bottom = true;
                }

                if !tail.is_empty() {
                    let mut queue = tail
                        .chunks(FILL_STREAM_CHUNK)
                        .map(<[ChatMessage]>::to_vec)
                        .collect::<VecDeque<Vec<ChatMessage>>>();
                    let chunk_sender = sender.command_sender().clone();
                    let stream_generation = generation;
                    let stream_identities = identities;

                    glib::idle_add_local(move || {
                        let Some(chunk) = queue.pop_front() else {
                            return glib::ControlFlow::Break;
                        };

                        chunk_sender.emit(ChatViewCommand::FillChunk {
                            generation: stream_generation,
                            messages: chunk,
                            identities: stream_identities.clone(),
                        });

                        if queue.is_empty() {
                            glib::ControlFlow::Break
                        } else {
                            glib::ControlFlow::Continue
                        }
                    });
                }
            }
            ChatViewCommand::FillChunk {
                generation,
                messages,
                identities: _,
            } => {
                if generation != self.generation {
                    return;
                }

                // The identity map was installed by the initial fill;
                // streaming chunks ride on it.
                self.history.prepend_messages(&messages);
            }
            ChatViewCommand::OlderMessagesLoaded {
                generation,
                messages,
                identities,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.set_identities(identities);
                self.history
                    .set_has_older(messages.len() == LOAD_MORE_COUNT);
                self.history.prepend_messages(&messages);
                self.state.is_loading = false;
            }
            ChatViewCommand::NewerMessagesLoaded {
                generation,
                messages,
                identities,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.set_identities(identities);

                // If fewer messages returned than requested, we've reached the real bottom.
                if messages.len() < LOAD_MORE_COUNT {
                    self.history.set_has_newer(false);
                }

                self.history.append_newer(&messages);
                self.state.is_loading = false;

                if self.state.is_at_bottom {
                    self.scroll_to_bottom(|| {});
                }
            }

            ChatViewCommand::JumpLoaded {
                generation,
                messages,
                identities,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.set_identities(identities);
                self.history.fill(&messages);
                self.history
                    .set_has_older(messages.len() == INITIAL_LOAD_COUNT);

                self.state.is_loading = false;

                // Scroll to the last message.
                self.scroll_to_bottom(|| {});
                self.state.is_at_bottom = true;
                self.state.unread_count = 0;
            }
            ChatViewCommand::FetchRetryTimeout { generation } => {
                // Give up on this attempt: the phone did not answer and
                // the queue slot has moved on. Release the marker so the
                // loading feedback goes away; scrolling back to the edge
                // re-requests, and a late answer still lands on its own.
                if generation != self.generation
                    || self.history_exhausted
                    || self
                        .history_fetch_at
                        .is_none_or(|armed| armed.elapsed() < HISTORY_FETCH_TIMEOUT)
                {
                    return;
                }

                self.history_fetch_at = None;
            }
            ChatViewCommand::BackfillLoaded {
                generation,
                older,
                newer,
                identities,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.set_identities(identities);

                // An anchored request that surfaced nothing older means the
                // phone has no history beyond the window: stop chaining.
                if self.history_fetch_anchored && older.is_empty() {
                    self.history_exhausted = true;
                }

                if !older.is_empty() {
                    self.history.set_has_older(older.len() == LOAD_MORE_COUNT);
                    self.history.prepend_messages(&older);
                }

                if !newer.is_empty() {
                    self.history.set_has_newer(newer.len() == LOAD_MORE_COUNT);
                    self.history.append_newer(&newer);

                    if self.state.is_at_bottom {
                        self.scroll_to_bottom(|| {});
                    }
                }

                self.state.is_loading = false;
            }
            ChatViewCommand::ScrollPositionChanged { at_top, at_bottom } => {
                self.state.is_at_top = at_top;

                if at_bottom != self.state.is_at_bottom {
                    self.state.is_at_bottom = at_bottom;
                    if at_bottom {
                        self.state.unread_count = 0;

                        // Arriving at the bottom after opening on a tall
                        // unread band marks the chat read.
                        if self.history.has_unread_messages()
                            && let Some(ref chat) = self.chat
                        {
                            let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                        }
                    }
                }

                // Guard against concurrent loads and exhausted history.
                if self.state.is_loading {
                    return;
                }

                if at_top
                    && self.history.has_older()
                    && let Some(ref chat) = self.chat
                    && let Some(before_ts) = self.history.oldest_timestamp()
                {
                    self.state.is_loading = true;

                    let db = self.db.clone();
                    let chat = chat.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let mut messages = chat
                            .load_messages_before(&db, before_ts, LOAD_MORE_COUNT)
                            .await
                            .unwrap_or_default();
                        let identities = re_resolve_senders(&db, &chat, &mut messages).await;

                        ChatViewCommand::OlderMessagesLoaded {
                            generation,
                            messages,
                            identities,
                        }
                    });
                } else if at_bottom
                    && self.history.has_newer()
                    && let Some(ref chat) = self.chat
                    && let Some(after_ts) = self.history.newest_timestamp()
                {
                    self.state.is_loading = true;

                    let db = self.db.clone();
                    let chat = chat.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let mut messages = chat
                            .load_messages_after(&db, after_ts, LOAD_MORE_COUNT)
                            .await
                            .unwrap_or_default();
                        let identities = re_resolve_senders(&db, &chat, &mut messages).await;

                        ChatViewCommand::NewerMessagesLoaded {
                            generation,
                            messages,
                            identities,
                        }
                    });
                } else if at_top {
                    // The local history is exhausted at the top: ask the
                    // phone for older messages on demand.
                    self.maybe_fetch_older_history(&sender);
                }
            }

            ChatViewCommand::TypingTimeout { generation } => {
                if generation == self.state.typing_generation
                    && self.state.is_typing
                    && let Some(ref chat) = self.chat
                {
                    self.state.is_typing = false;
                    let _ = sender.output(ChatViewOutput::TypingStateChanged {
                        chat_jid: chat.jid.clone(),
                        composing: false,
                    });
                }
            }
        }
    }
}

impl ChatView {
    /// Whether an on-demand history request is waiting for the phone,
    /// within its retry window.
    fn is_fetching_history(&self) -> bool {
        self.history_fetch_at
            .is_some_and(|at| at.elapsed() < HISTORY_FETCH_RETRY)
    }

    /// Ask the phone for the history older than the window, when the view
    /// sits at the top of an exhausted local history.
    fn maybe_fetch_older_history(&mut self, sender: &AsyncComponentSender<Self>) {
        if !self.state.is_at_top
            || self.state.is_loading
            || self.history_exhausted
            || self.history.has_older()
            || self.is_fetching_history()
        {
            return;
        }

        if let Some(ref chat) = self.chat
            && !is_status_broadcast(&chat.jid)
        {
            let anchor = self.oldest_message_anchor();
            self.history_fetch_at = Some(Instant::now());
            self.history_fetch_anchored = anchor.is_some();

            let _ = sender.output(ChatViewOutput::FetchHistory {
                chat_jid: chat.jid.clone(),
                anchor,
            });
            self.schedule_fetch_timeout(sender);
        }
    }

    /// Re-sends an unanswered request every `HISTORY_FETCH_TIMEOUT` while
    /// the chat stays open, since the phone app drops them while asleep.
    fn schedule_fetch_timeout(&self, sender: &AsyncComponentSender<Self>) {
        let generation = self.generation;
        let command_sender = sender.command_sender().clone();
        glib::timeout_add_local_once(HISTORY_FETCH_TIMEOUT, move || {
            command_sender.emit(ChatViewCommand::FetchRetryTimeout { generation });
        });
    }

    /// Anchor for an on-demand history request, taken from the oldest
    /// message row in view. Rows without a server id cannot anchor, and
    /// neither can a window without messages.
    fn oldest_message_anchor(&self) -> Option<HistoryAnchor> {
        for index in 0..self.history.len() {
            if let Some(ChatRow::Message { message, .. }) = self.history.get_row(index) {
                if message.server_id.is_empty() {
                    continue;
                }

                return Some(HistoryAnchor {
                    from_me: message.outgoing,
                    server_id: message.server_id,
                    timestamp_ms: message.timestamp.as_millisecond(),
                });
            }
        }

        None
    }

    fn update_presence(&mut self) {
        if let Some(ref mut chat) = self.chat {
            if chat.is_group() {
                self.state.presence = Self::participants_label(chat);
            } else if chat.available.unwrap_or_default() {
                self.state.presence = Some(i18n!("online"));
            } else if let Some(last_seen) = chat.last_seen {
                let today = Zoned::now().date();
                let last_seen_local = last_seen.to_zoned(TimeZone::system());
                let last_date = last_seen_local.date();

                let presence = if last_date == today {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        i18n!("today"),
                        i18n!("at"),
                        last_seen_local.strftime("%H:%M")
                    )
                } else if let Some(yesterday) = today.yesterday().ok()
                    && last_date == yesterday
                {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        i18n!("yesterday"),
                        i18n!("at"),
                        last_seen_local.strftime("%H:%M")
                    )
                } else {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        last_date.strftime("%d/%m"),
                        i18n!("at"),
                        last_seen_local.strftime("%H:%M")
                    )
                };
                self.state.presence = Some(presence);
            }
        }
    }

    fn participants_label(chat: &Chat) -> Option<String> {
        let unknown = i18n!("Unknown");
        let mut names = chat
            .participants
            .values()
            .filter(|name| !name.is_empty() && name.as_str() != unknown.as_str())
            .cloned()
            .collect::<Vec<String>>();
        names.sort();
        names.dedup();

        if names.is_empty() {
            None
        } else if names.len() > 3 {
            Some(format!(
                "{}, {}",
                names[..3].join(", "),
                i18n_f!("+{0} more", names.len() - 3)
            ))
        } else {
            Some(names.join(", "))
        }
    }

    fn sync_banner_label(&self) -> String {
        let Some(sync) = self.state.sync.as_ref() else {
            return i18n!("Syncing messages...");
        };

        match (sync.percent, sync.total) {
            (Some(percent), _) => i18n_f!("Syncing messages... {0}%", percent),
            (None, total) if total > 0 => {
                i18n_f!("Syncing messages... {0} of {1} chats", sync.synced, total)
            }
            _ => i18n!("Syncing messages..."),
        }
    }

    fn banner_sync_fraction(&self) -> Option<f64> {
        self.state
            .sync
            .as_ref()
            .and_then(|sync| sync.percent)
            .map(|percent| f64::from(percent) / 100.0)
    }

    fn unread_badge_label(&self) -> String {
        if self.state.unread_count > 99 {
            "99+".to_string()
        } else {
            self.state.unread_count.to_string()
        }
    }

    fn typing_text(&self) -> String {
        let recording = self.state.typing.iter().any(|s| s.recording);
        if self.state.typing.len() == 1 && recording {
            i18n_f!(
                "{0} is recording audio...",
                self.state.typing[0].name.clone()
            )
        } else if self.state.typing.len() == 1 {
            i18n_f!("{0} is typing...", self.state.typing[0].name.clone())
        } else if recording {
            i18n_f!("{0} people are recording audio...", self.state.typing.len())
        } else {
            i18n_f!("{0} people are typing...", self.state.typing.len())
        }
    }

    /// The line under the header title: participant names for groups,
    /// peer presence for private chats.
    fn header_subtitle(&self) -> Option<String> {
        let chat = self.chat.as_ref()?;

        if chat.is_group() {
            let mut names = chat
                .participants
                .values()
                .filter(|name| name.as_str() != "Unknown")
                .cloned()
                .collect::<Vec<String>>();
            names.sort_by_key(|name| name.to_lowercase());
            if names.is_empty() {
                return None;
            }

            let shown = names.iter().take(4).cloned().collect::<Vec<String>>();
            Some(if names.len() > 4 {
                format!("{}, +{}", shown.join(", "), names.len() - 4)
            } else {
                shown.join(", ")
            })
        } else {
            self.state.presence.clone()
        }
    }

    fn rebuild_typing_avatars(&self) {
        while let Some(child) = self.typing_avatars.first_child() {
            self.typing_avatars.remove(&child);
        }

        let is_group = self.chat.as_ref().is_some_and(Chat::is_group);
        self.typing_avatars.set_visible(is_group);

        for sender in &self.state.typing {
            let avatar = adw::Avatar::builder().size(24).show_initials(true).build();
            avatar.set_text(
                (!sender.name.starts_with('+') && sender.name != i18n!("Someone"))
                    .then_some(sender.name.as_str()),
            );
            avatar.set_widget_name(&sender.jid);

            if let Some(path) = avatar::sender_cached_path(&sender.jid) {
                if let Some(texture) = avatar::texture(&path) {
                    avatar.set_custom_image(Some(&texture));
                } else {
                    let name = sender.jid.clone();
                    let path = path.clone();
                    let avatar = avatar.clone();

                    glib::spawn_future_local(async move {
                        if let Some(texture) = avatar::load(&path).await
                            && avatar.widget_name() == name
                        {
                            avatar.set_custom_image(Some(&texture));
                        }
                    });
                }
            }

            let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            frame.append(&avatar);

            self.typing_avatars.append(&frame);
        }
    }

    fn scroll_to_bottom(&self, on_settled: impl FnOnce() + 'static) {
        self.history.scroll_to_bottom(on_settled);
    }
}
