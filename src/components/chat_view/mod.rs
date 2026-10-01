mod history;
mod momentum;
mod rows;

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use adw::prelude::*;
use gtk::{gdk, glib, pango};
use jiff::{Timestamp, Zoned, tz::TimeZone};
use relm4::prelude::*;
use tokio::time;
use uuid::Uuid;

use self::{history::ChatHistory, momentum::Momentum, rows::ChatRow};
use crate::{
    db::store::SessionStore,
    i18n, i18n_f,
    state::{Chat, ChatMessage, HistoryAnchor, MessageStatus, TypingSender},
    widgets::TypingDots,
};

const LOAD_MORE_COUNT: usize = 70;
/// Rows of read context kept above the unread band when opening on it.
const UNREAD_BAND_CONTEXT_ROWS: u32 = 6;
/// Above this many unread rows, opening positions at the band top instead of
/// scrolling to the bottom, deferring the read mark until the user arrives.
const UNREAD_BAND_BOTTOM_MAX_ROWS: usize = 8;
/// Maximum number of rows (messages + separators) to keep loaded.
const MAX_LOADED_ROWS: u32 = 600;
const INITIAL_LOAD_COUNT: usize = 120;
/// Cooldown between on-demand history requests for one chat.
const HISTORY_FETCH_RETRY: Duration = Duration::from_secs(60);
/// How long to wait for a phone answer before asking again. Requests
/// made while the phone app is asleep are dropped rather than queued.
const HISTORY_FETCH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub struct ChatView {
    db: SessionStore,
    chat: Option<Chat>,
    state: ChatViewState,
    history: ChatHistory,
    momentum: Momentum,
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

#[derive(Debug)]
pub enum ChatViewInput {
    Open(Chat),
    Close,

    SendMessage,
    EntryChanged,
    MessageReceived(Box<ChatMessage>),

    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<Timestamp>,
    },
    TypingUpdate {
        chat_jid: String,
        senders: Vec<TypingSender>,
    },
    MessageStatusUpdate {
        status: MessageStatus,
        local_id: Uuid,
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
}

#[derive(Debug)]
pub enum ChatViewCommand {
    InitialMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        had_unread: bool,
    },
    OlderMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },
    NewerMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },

    JumpLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },
    FetchRetryTimeout {
        generation: u64,
    },
    BackfillLoaded {
        generation: u64,
        older: Vec<ChatMessage>,
        newer: Vec<ChatMessage>,
    },

    ScrollSettled {
        generation: u64,
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
                            #[watch]
                            set_label?: model.chat.as_ref().map(Chat::get_name_or_number).as_ref(),
                            #[watch]
                            set_visible: model.chat.is_some(),
                            set_selectable: false,
                            set_css_classes: &["title"],
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
                        set_css_classes: &["service-message", "card"],
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
        let history = ChatHistory::new();

        let model = Self {
            db,
            chat: None,
            state: ChatViewState {
                sync: None,
                typing: Vec::new(),
                presence: None,
                is_loading: true,
                is_typing: false,
                is_at_top: false,
                is_at_bottom: true,
                unread_count: 0,
                typing_generation: 0,
            },
            history,
            momentum: Momentum::new(),
            generation: 0,
            message_entry: gtk::Entry::new(),
            typing_avatars: gtk::Box::new(gtk::Orientation::Horizontal, 0),
            history_fetch_at: None,
            history_exhausted: false,
            history_fetch_anchored: false,
        };

        let list_view = model.history.view().view.clone();
        let scroll_window = gtk::ScrolledWindow::new();
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
        model.momentum.attach(&scroll_window);

        let momentum = model.momentum.clone();
        let scroll_controller =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);

        let stop_momentum = momentum.clone();
        scroll_controller.connect_scroll(move |_, _, _| {
            stop_momentum.stop();
            glib::Propagation::Proceed
        });
        scroll_window.add_controller(scroll_controller);

        let command_sender = sender.command_sender().clone();
        let was_at_top = Rc::new(Cell::new(false));
        let was_at_bottom = Rc::new(Cell::new(true));
        adj.connect_value_changed(move |adj| {
            if momentum.record(adj.value()) {
                momentum.take_over_down();
            }

            let at_top = adj.value() <= 50.0 && adj.upper() > adj.page_size();
            let at_bottom = adj.value() + adj.page_size() >= adj.upper() - 25.0;

            // Trigger load of older messages when scrolled near the top.
            if at_top {
                was_at_bottom.set(false);

                if !was_at_top.get() {
                    was_at_top.set(true);
                    command_sender.emit(ChatViewCommand::ScrollPositionChanged {
                        at_top: true,
                        at_bottom: false,
                    });
                }
            } else {
                // Leaving the top matters as much as arriving: on-demand
                // fetches are level-checked against the tracked position.
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

    #[allow(clippy::unused_async_trait_impl)]
    #[allow(clippy::too_many_lines)]
    async fn update(
        &mut self,
        input: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            ChatViewInput::Open(chat) => {
                self.generation += 1;

                self.momentum.stop();
                self.history.clear();

                if self.state.is_typing
                    && let Some(ref old) = self.chat
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

                // Update the user presence label.
                self.update_presence();

                // Grab message entry focus as convenience.
                self.message_entry.grab_focus();

                // Load the initial batch of messages.
                let db = self.db.clone();
                let generation = self.generation;
                sender.oneshot_command(async move {
                    let messages = chat
                        .load_messages(&db, INITIAL_LOAD_COUNT)
                        .await
                        .unwrap_or_default();
                    let had_unread = chat
                        .get_unread_count(&db)
                        .await
                        .is_ok_and(|count| count > 0);
                    ChatViewCommand::InitialMessagesLoaded {
                        generation,
                        messages,
                        had_unread,
                    }
                });

                let _ = sender.output(ChatViewOutput::ChatOpen);
            }
            ChatViewInput::Close => {
                self.generation += 1;

                self.momentum.stop();
                self.history.clear();

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
            ChatViewInput::TypingUpdate { chat_jid, senders } => {
                if self.chat.as_ref().is_none_or(|chat| chat.jid != chat_jid) {
                    return;
                }

                self.state.typing = senders;
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
                        let messages = chat
                            .load_messages(&db, INITIAL_LOAD_COUNT)
                            .await
                            .unwrap_or_default();

                        ChatViewCommand::InitialMessagesLoaded {
                            generation,
                            messages,
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
                        let older = match before_ts {
                            Some(before_ts) => chat
                                .load_messages_before(&db, before_ts, LOAD_MORE_COUNT)
                                .await
                                .unwrap_or_default(),
                            None => Vec::new(),
                        };
                        let newer = match after_ts {
                            Some(after_ts) => chat
                                .load_messages_after(&db, after_ts, LOAD_MORE_COUNT)
                                .await
                                .unwrap_or_default(),
                            None => Vec::new(),
                        };

                        ChatViewCommand::BackfillLoaded {
                            generation,
                            older,
                            newer,
                        }
                    });
                }
            }

            ChatViewInput::ScrollToBottom => {
                // If either end has been trimmed, the view is a "window" into the
                // message history — reload from scratch to jump to the real latest.
                if self.history.has_newer() || self.history.has_older() {
                    self.generation += 1;
                    self.momentum.stop();
                    self.history.clear();
                    self.state.is_loading = true;

                    if let Some(ref chat) = self.chat {
                        let db = self.db.clone();
                        let chat = chat.clone();
                        let generation = self.generation;
                        sender.oneshot_command(async move {
                            let messages = chat
                                .load_messages(&db, INITIAL_LOAD_COUNT)
                                .await
                                .unwrap_or_default();
                            ChatViewCommand::JumpLoaded {
                                generation,
                                messages,
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

    #[allow(clippy::unused_async_trait_impl)]
    #[allow(clippy::too_many_lines)]
    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            ChatViewCommand::InitialMessagesLoaded {
                generation,
                messages,
                had_unread,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history.fill(&messages);
                self.history
                    .set_has_older(messages.len() == INITIAL_LOAD_COUNT);

                self.state.is_loading = false;

                // A chat whose stored history is shorter than a page may be
                // missing older messages, including chats too short to
                // scroll: ask the phone for the history before the oldest
                // stored message.
                if messages.len() < INITIAL_LOAD_COUNT
                    && let Some(ref chat) = self.chat
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

                // A tall unread band opens at its top, with context rows
                // above; the view marks the chat read above already.
                let tall_unread_band =
                    self.history.unread_message_row_count() > UNREAD_BAND_BOTTOM_MAX_ROWS;

                if tall_unread_band && let Some(divider) = self.history.unread_divider_index() {
                    self.history.scroll_to_unread_boundary(
                        divider.saturating_sub(UNREAD_BAND_CONTEXT_ROWS),
                    );
                    self.state.is_at_bottom = false;
                } else {
                    // Scroll to the last message.
                    self.scroll_to_bottom(|| {});
                    self.state.is_at_bottom = true;
                }
            }
            ChatViewCommand::OlderMessagesLoaded {
                generation,
                messages,
            } => {
                if generation != self.generation {
                    return;
                }

                self.history
                    .set_has_older(messages.len() == LOAD_MORE_COUNT);

                let (baseline, velocity) = self.momentum.capture();

                self.history.prepend_messages(&messages);

                // Trim excess rows from the bottom to stay within MAX_LOADED_ROWS.
                self.history.trim_bottom(MAX_LOADED_ROWS);

                self.momentum.continue_from(baseline, velocity);

                let command_sender = sender.command_sender().clone();
                glib::idle_add_local_once(move || {
                    command_sender.emit(ChatViewCommand::ScrollSettled { generation });
                });
            }
            ChatViewCommand::NewerMessagesLoaded {
                generation,
                messages,
            } => {
                if generation != self.generation {
                    return;
                }

                // If fewer messages returned than requested, we've reached the real bottom.
                if messages.len() < LOAD_MORE_COUNT {
                    self.history.set_has_newer(false);
                }

                self.history.append_newer(&messages);

                // Trim excess rows from the top to stay within MAX_LOADED_ROWS.
                self.history.trim_top(MAX_LOADED_ROWS);

                if self.state.is_at_bottom {
                    self.scroll_to_bottom(|| {});
                }

                let command_sender = sender.command_sender().clone();
                glib::idle_add_local_once(move || {
                    command_sender.emit(ChatViewCommand::ScrollSettled { generation });
                });
            }

            ChatViewCommand::JumpLoaded {
                generation,
                messages,
            } => {
                if generation != self.generation {
                    return;
                }

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
                // The request was answered or replaced if the marker moved
                // on; only a timed-out pending request is re-sent.
                if generation != self.generation
                    || self.history_exhausted
                    || self
                        .history_fetch_at
                        .is_none_or(|armed| armed.elapsed() < HISTORY_FETCH_TIMEOUT)
                {
                    return;
                }

                if let Some(ref chat) = self.chat {
                    self.history_fetch_at = Some(Instant::now());

                    let _ = sender.output(ChatViewOutput::FetchHistory {
                        chat_jid: chat.jid.clone(),
                        anchor: self
                            .history_fetch_anchored
                            .then(|| self.oldest_message_anchor())
                            .flatten(),
                    });
                    self.schedule_fetch_timeout(&sender);
                }
            }
            ChatViewCommand::BackfillLoaded {
                generation,
                older,
                newer,
            } => {
                if generation != self.generation {
                    return;
                }

                // An anchored request that surfaced nothing older means the
                // phone has no history beyond the window: stop chaining.
                if self.history_fetch_anchored && older.is_empty() {
                    self.history_exhausted = true;
                }

                if !older.is_empty() {
                    self.history.set_has_older(older.len() == LOAD_MORE_COUNT);

                    let (baseline, velocity) = self.momentum.capture();

                    self.history.prepend_messages(&older);

                    // Trim excess rows from the bottom to stay within MAX_LOADED_ROWS.
                    self.history.trim_bottom(MAX_LOADED_ROWS);

                    self.momentum.continue_from(baseline, velocity);
                }

                if !newer.is_empty() {
                    self.history.set_has_newer(newer.len() == LOAD_MORE_COUNT);

                    self.history.append_newer(&newer);

                    // Trim excess rows from the top to stay within MAX_LOADED_ROWS.
                    self.history.trim_top(MAX_LOADED_ROWS);

                    if self.state.is_at_bottom {
                        self.scroll_to_bottom(|| {});
                    }
                }

                self.state.is_loading = false;

                let command_sender = sender.command_sender().clone();
                glib::idle_add_local_once(move || {
                    command_sender.emit(ChatViewCommand::ScrollSettled { generation });
                });
            }

            ChatViewCommand::ScrollSettled { generation } => {
                if generation == self.generation {
                    self.state.is_loading = false;
                    self.maybe_fetch_older_history(&sender);
                }
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
                        let messages = chat
                            .load_messages_before(&db, before_ts, LOAD_MORE_COUNT)
                            .await
                            .unwrap_or_default();
                        ChatViewCommand::OlderMessagesLoaded {
                            generation,
                            messages,
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
                        let messages = chat
                            .load_messages_after(&db, after_ts, LOAD_MORE_COUNT)
                            .await
                            .unwrap_or_default();
                        ChatViewCommand::NewerMessagesLoaded {
                            generation,
                            messages,
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
    ///
    /// Level-checked from both scroll events and settle callbacks, so
    /// history landing while the view is parked at the top keeps the
    /// requests going without further scrolling. `history_fetch_at` marks
    /// a request in flight and paces retries: a backfill response clears
    /// it and the retry window re-arms it.
    fn maybe_fetch_older_history(&mut self, sender: &AsyncComponentSender<Self>) {
        if !self.state.is_at_top
            || self.state.is_loading
            || self.history_exhausted
            || self.history.has_older()
            || self.is_fetching_history()
        {
            return;
        }

        if let Some(ref chat) = self.chat {
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

    fn rebuild_typing_avatars(&self) {
        while let Some(child) = self.typing_avatars.first_child() {
            self.typing_avatars.remove(&child);
        }

        let is_group = self.chat.as_ref().is_some_and(Chat::is_group);
        self.typing_avatars.set_visible(is_group);

        if !is_group {
            return;
        }

        for sender in &self.state.typing {
            let avatar = adw::Avatar::builder().size(24).show_initials(true).build();
            avatar.set_text(
                (!sender.name.starts_with('+') && sender.name != i18n!("Someone"))
                    .then_some(sender.name.as_str()),
            );

            let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            frame.append(&avatar);

            self.typing_avatars.append(&frame);
        }
    }

    fn scroll_to_bottom(&self, on_settled: impl FnOnce() + 'static) {
        self.momentum.stop();
        self.momentum.pause_recording();
        self.history.scroll_to_bottom(on_settled);
    }
}
