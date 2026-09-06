mod history;
mod momentum;
mod rows;

use std::{cell::Cell, rc::Rc};

use adw::prelude::*;
use chrono::{DateTime, Local, Utc};
use gtk::{gdk, glib};
use relm4::prelude::*;
use uuid::Uuid;

use self::{history::ChatHistory, momentum::Momentum, rows::ChatRow};
use crate::{
    i18n,
    state::{Chat, ChatMessage, MessageStatus},
};

/// Number of messages to load when scrolling.
const LOAD_MORE_COUNT: u32 = 70;
/// Maximum number of rows (messages + separators) to keep loaded.
const MAX_LOADED_ROWS: u32 = 600;
/// Number of messages to load on initial chat open.
const INITIAL_LOAD_COUNT: u32 = 120;

#[derive(Debug)]
pub struct ChatView {
    /// Currently open chat.
    chat: Option<Chat>,
    /// Current chat view state.
    state: ChatViewState,
    /// Owned message list + pagination state.
    history: ChatHistory,
    /// Touchpad flick continuation across prepended batches.
    momentum: Momentum,
    /// Monotonic generation counter, incremented on every chat open or jump
    /// reload. Used to discard stale command results from a previous chat.
    generation: u64,
    /// Text input for sending messages.
    message_entry: gtk::Entry,
}

#[derive(Debug)]
pub struct ChatViewState {
    /// User presence.
    presence: Option<String>,
    /// Whether a load operation is currently in progress.
    is_loading: bool,
    /// Whether the scroll is at the bottom.
    is_at_bottom: bool,
}

#[derive(Debug)]
pub enum ChatViewInput {
    /// Open a chat.
    Open(Chat),
    /// Close the open chat.
    Close,

    /// Send a message.
    SendMessage,
    /// New message received.
    MessageReceived(Box<ChatMessage>),

    /// User presence updated.
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<DateTime<Utc>>,
    },
    /// Message status updated.
    MessageStatusUpdate {
        status: MessageStatus,
        local_id: Uuid,
    },

    /// Scroll to the bottom of the chat.
    ScrollToBottom,
}

#[derive(Debug)]
pub enum ChatViewOutput {
    /// A chat was open.
    ChatOpen,
    /// The open chat was closed.
    ChatClosed,
    /// Mark the open chat as read.
    MarkChatRead(String),

    /// Send a text message.
    SendTextMessage {
        /// The content of the message.
        text: String,
        /// Message recipient.
        recipient: String,
    },
}

#[derive(Debug)]
pub enum ChatViewCommand {
    /// Initial batch of messages loaded for a newly opened chat.
    InitialMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
        had_unread: bool,
    },
    /// Older messages loaded for upward pagination.
    OlderMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },
    /// Newer messages loaded for downward pagination.
    NewerMessagesLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },
    /// Fresh batch loaded for a jump-to-bottom reload.
    JumpLoaded {
        generation: u64,
        messages: Vec<ChatMessage>,
    },

    /// Scroll anchoring finished after a prepend-driven reallocation.
    ScrollSettled { generation: u64 },
    /// The scroll position has changed.
    ScrollPositionChanged { at_top: bool, at_bottom: bool },
}

#[relm4::component(async, pub)]
impl AsyncComponent for ChatView {
    type Init = ();
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
                            set_css_classes: &["subtitle"],
                        },
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
                    set_reveal_child: model.chat.is_some() && model.state.is_loading,
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

                    gtk::Button {
                        set_icon_name: "down-small-symbolic",
                        set_css_classes: &["circular", "osd"],
                        set_margin_bottom: 12,

                        connect_clicked => ChatViewInput::ScrollToBottom
                    },
                },
            },

            add_bottom_bar = &gtk::Box {
                set_spacing: 6,
                set_margin_all: 6,
                set_orientation: gtk::Orientation::Horizontal,

                #[local_ref]
                message_entry -> gtk::Entry {
                    set_hexpand: true,
                    set_placeholder_text: Some(&i18n!("Type a message...")),

                    connect_activate => ChatViewInput::SendMessage,
                },

                gtk::Button {
                    set_icon_name: "paper-plane-symbolic",
                    set_css_classes: &["circular", "suggested-action"],

                    connect_clicked => ChatViewInput::SendMessage,
                },
            },
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let history = ChatHistory::new();

        let model = Self {
            chat: None,
            state: ChatViewState {
                presence: None,
                is_loading: true,
                is_at_bottom: true,
            },
            history,
            momentum: Momentum::new(),
            generation: 0,
            message_entry: gtk::Entry::new(),
        };

        let list_view = model.history.view().view.clone();
        let scroll_window = gtk::ScrolledWindow::new();
        let message_entry = &model.message_entry;
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
                was_at_top.set(false);

                if at_bottom != was_at_bottom.get() {
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

                // Reset state.
                self.state.presence = None;
                self.state.is_loading = true;
                self.state.is_at_bottom = true;

                self.chat = Some(chat.clone());

                // Update the user presence label.
                self.update_presence();

                // Grab message entry focus as convenience.
                self.message_entry.grab_focus();

                // Load the initial batch of messages.
                let generation = self.generation;
                sender.oneshot_command(async move {
                    let messages = chat
                        .load_messages(INITIAL_LOAD_COUNT)
                        .await
                        .unwrap_or_default();
                    let had_unread = chat.get_unread_count().await.is_ok_and(|count| count > 0);
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

                // Reset state.
                self.chat = None;
                self.state.presence = None;
                self.state.is_loading = false;
                self.state.is_at_bottom = false;

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

                    // Scroll the chat to bottom and mark it as read.
                    sender.input(ChatViewInput::ScrollToBottom);
                    let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                }
            }
            ChatViewInput::MessageReceived(message) => {
                // If the bottom has been trimmed, skip appending — the message will
                // appear when the user scrolls back to bottom and triggers a reload.
                if self.history.has_newer() {
                    return;
                }

                self.history.append_live(*message);

                if self.state.is_at_bottom {
                    self.scroll_to_bottom();
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
            ChatViewInput::MessageStatusUpdate { local_id, status } => {
                if let Some(index) = self
                    .history
                    .find_message_index(|message| message.local_id == local_id)
                    && let Some(mut row) = self.history.get_row(index)
                {
                    if let ChatRow::Message(message) = &mut row {
                        message.status = status;
                    }

                    self.history
                        .replace_row(index, row, self.state.is_at_bottom);
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
                        let chat = chat.clone();
                        let generation = self.generation;
                        sender.oneshot_command(async move {
                            let messages = chat
                                .load_messages(INITIAL_LOAD_COUNT)
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
                    self.scroll_to_bottom();
                    self.state.is_at_bottom = true;
                }
            }
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
                    .set_has_older(messages.len() == usize::try_from(INITIAL_LOAD_COUNT).unwrap());

                self.state.is_loading = false;

                // Scroll to the last message.
                self.scroll_to_bottom();
                self.state.is_at_bottom = true;

                if had_unread && let Some(ref chat) = self.chat {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
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
                    .set_has_older(messages.len() == usize::try_from(LOAD_MORE_COUNT).unwrap());

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
                if messages.len() < usize::try_from(LOAD_MORE_COUNT).unwrap() {
                    self.history.set_has_newer(false);
                }

                self.history.append_newer(&messages);

                // Trim excess rows from the top to stay within MAX_LOADED_ROWS.
                self.history.trim_top(MAX_LOADED_ROWS);

                if self.state.is_at_bottom {
                    self.scroll_to_bottom();
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
                    .set_has_older(messages.len() == usize::try_from(INITIAL_LOAD_COUNT).unwrap());

                self.state.is_loading = false;

                // Scroll to the last message.
                self.scroll_to_bottom();
                self.state.is_at_bottom = true;
            }

            ChatViewCommand::ScrollSettled { generation } => {
                if generation == self.generation {
                    self.state.is_loading = false;
                }
            }
            ChatViewCommand::ScrollPositionChanged { at_top, at_bottom } => {
                if at_bottom != self.state.is_at_bottom {
                    self.state.is_at_bottom = at_bottom;
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
                    let chat = chat.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let messages = chat
                            .load_messages_before(before_ts, LOAD_MORE_COUNT)
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
                    let chat = chat.clone();
                    let generation = self.generation;
                    sender.oneshot_command(async move {
                        let messages = chat
                            .load_messages_after(after_ts, LOAD_MORE_COUNT)
                            .await
                            .unwrap_or_default();
                        ChatViewCommand::NewerMessagesLoaded {
                            generation,
                            messages,
                        }
                    });
                }
            }
        }
    }
}

impl ChatView {
    /// Update the user presence.
    fn update_presence(&mut self) {
        if let Some(ref mut chat) = self.chat {
            if chat.available.unwrap_or_default() {
                self.state.presence = Some(i18n!("online"));
            } else if let Some(last_seen) = chat.last_seen {
                let today = Local::now().date_naive();
                let last_date = last_seen.with_timezone(&Local).date_naive();

                let presence = if last_date == today {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        i18n!("today"),
                        i18n!("at"),
                        last_date.format("%H:%M")
                    )
                } else if let Some(yesterday) = today.pred_opt()
                    && last_date == yesterday
                {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        i18n!("yesterday"),
                        i18n!("at"),
                        last_date.format("%H:%M")
                    )
                } else {
                    format!(
                        "{} {} {} {}",
                        i18n!("Last seen"),
                        last_date.format("%d/%m"),
                        i18n!("at"),
                        last_date.format("%H:%M")
                    )
                };
                self.state.presence = Some(presence);
            }
        }
    }

    fn scroll_to_bottom(&self) {
        self.momentum.stop();
        self.momentum.pause_recording();
        self.history.scroll_to_bottom();
    }
}
