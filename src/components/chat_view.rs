use std::{cell::Cell, collections::VecDeque, rc::Rc};

use adw::{gdk, glib, prelude::*};
use chrono::{DateTime, Local, NaiveDate, Utc};
use gtk::pango;
use relm4::{
    prelude::*,
    typed_view::list::{RelmListItem, TypedListView},
};

use crate::{
    i18n,
    state::{Chat, ChatMessage},
    utils::format_date_label,
};

/// Number of messages to load when scrolling to the top.
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
    /// Metadata tracking for each row, mirrors list_view_wrapper order.
    /// Used to update pagination cursors when trimming rows.
    row_metadata: VecDeque<RowMetadata>,
    /// Text input for sending messages.
    message_entry: gtk::Entry,
    /// `ListView` widget wrapper containing all chat rows.
    list_view_wrapper: TypedListView<ChatRow, gtk::NoSelection>,
}

/// Metadata for a single row in the chat list, used for cursor tracking
/// when trimming rows during bidirectional pagination.
#[derive(Clone, Debug)]
enum RowMetadata {
    /// A message row, with its Unix timestamp.
    Message(i64),
    /// A date separator row.
    Separator(NaiveDate),
}

#[derive(Debug)]
pub struct ChatViewState {
    /// Whether a load operation is currently in progress.
    is_loading: bool,
    /// Whether the scroll is at the bottom.
    is_at_bottom: bool,
    /// Whether messages at the top have been trimmed due to exceeding MAX_LOADED_ROWS.
    top_trimmed: bool,
    /// Whether messages at the bottom have been trimmed due to exceeding MAX_LOADED_ROWS.
    bottom_trimmed: bool,
    /// Whether there might be more older messages to load.
    has_more_messages: bool,

    /// User presence.
    presence: Option<String>,
    /// Date of the first displayed message (top).
    first_message_date: Option<NaiveDate>,
    /// Date of the last appended message (bottom).
    last_message_date: Option<NaiveDate>,
    /// Timestamp of the oldest loaded message.
    oldest_loaded_timestamp: Option<i64>,
    /// Timestamp of the newest loaded message.
    newest_loaded_timestamp: Option<i64>,
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
    MessageReceived(ChatMessage),

    /// User presence updated.
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<DateTime<Utc>>,
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
}

#[derive(Debug)]
pub enum ChatViewCommand {
    /// Load older messages when the user scrolls to the top.
    LoadOlderMessages,
    /// Load newer messages when the user scrolls to the bottom.
    LoadNewerMessages,

    /// The scroll position has changed.
    ScrollPositionChanged { at_top: bool, at_bottom: bool },
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
                            set_label?: model.chat.as_ref().map(|c| c.get_name_or_number()).as_ref(),
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
                    set_propagate_natural_width: true,

                    adw::ClampScrollable {
                        set_maximum_size: 800,
                        set_vscroll_policy: gtk::ScrollablePolicy::Natural,
                        set_tightening_threshold: 600,

                        #[local_ref]
                        list_view -> gtk::ListView {
                            set_css_classes: &["chat-history"],
                            set_single_click_activate: false
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
        let list_view_wrapper = TypedListView::new();

        let model = Self {
            chat: None,
            state: ChatViewState {
                is_loading: true,
                is_at_bottom: true,
                top_trimmed: true,
                bottom_trimmed: false,
                has_more_messages: false,

                presence: None,
                first_message_date: None,
                last_message_date: None,
                oldest_loaded_timestamp: None,
                newest_loaded_timestamp: None,
            },
            row_metadata: VecDeque::new(),
            message_entry: gtk::Entry::new(),
            list_view_wrapper,
        };

        let list_view = &model.list_view_wrapper.view;
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
        let command_sender = sender.command_sender().clone();
        let was_at_top = Rc::new(Cell::new(false));
        let was_at_bottom = Rc::new(Cell::new(true));
        adj.connect_value_changed(move |adj| {
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

    async fn update(
        &mut self,
        input: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            ChatViewInput::Open(chat) => {
                self.row_metadata.clear();
                self.list_view_wrapper.clear();

                // Reset state.
                self.state.presence = None;
                self.state.is_loading = true;
                self.state.top_trimmed = true;
                self.state.bottom_trimmed = false;
                self.state.first_message_date = None;
                self.state.last_message_date = None;
                self.state.oldest_loaded_timestamp = None;
                self.state.newest_loaded_timestamp = None;

                let jid = chat.jid.clone();

                // Load the initial batch of messages.
                if let Ok(messages) = chat.load_messages(INITIAL_LOAD_COUNT).await {
                    self.state.has_more_messages = messages.len() as u32 == INITIAL_LOAD_COUNT;

                    // Track the oldest loaded timestamp for pagination.
                    if let Some(oldest) = messages.last() {
                        self.state.oldest_loaded_timestamp = Some(oldest.timestamp.timestamp());
                    }

                    // Track the newest loaded timestamp for downward pagination.
                    if let Some(newest) = messages.first() {
                        self.state.newest_loaded_timestamp = Some(newest.timestamp.timestamp());
                    }

                    for msg in messages.iter().rev() {
                        // Convert to local date for separator comparison.
                        let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

                        // Insert a date separator if the date changed.
                        if self.state.last_message_date.map_or(true, |d| d != msg_date) {
                            self.list_view_wrapper
                                .append(ChatRow::DateSeparator(msg_date));
                            self.row_metadata
                                .push_back(RowMetadata::Separator(msg_date));
                            self.state.last_message_date = Some(msg_date);
                        }

                        // Track the first message date for prepend separators.
                        if self.state.first_message_date.is_none() {
                            self.state.first_message_date = Some(msg_date);
                        }

                        self.list_view_wrapper.append(ChatRow::Message(msg.clone()));
                        self.row_metadata
                            .push_back(RowMetadata::Message(msg.timestamp.timestamp()));
                    }

                    // Scroll to the last message.
                    let count = self.list_view_wrapper.len();
                    if count > 0 {
                        let info = gtk::ScrollInfo::new();
                        info.set_enable_vertical(true);
                        self.list_view_wrapper.view.scroll_to(
                            (count - 1) as u32,
                            gtk::ListScrollFlags::FOCUS,
                            Some(info),
                        );

                        self.state.is_at_bottom = true;
                    }
                }

                // Mark chat as read if it has unread messages.
                if chat.get_unread_count().await.is_ok_and(|count| count > 0) {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(jid));
                }

                // Update the user presence label.
                self.update_presence();

                // Grab message entry focus as convenience.
                self.message_entry.grab_focus();

                self.chat = Some(chat);
                self.state.is_loading = false;

                let _ = sender.output(ChatViewOutput::ChatOpen);
            }
            ChatViewInput::Close => {
                self.row_metadata.clear();
                self.list_view_wrapper.clear();

                // Reset state.
                self.chat = None;
                self.state.presence = None;
                self.state.is_loading = false;
                self.state.top_trimmed = false;
                self.state.is_at_bottom = false;
                self.state.bottom_trimmed = false;
                self.state.first_message_date = None;
                self.state.last_message_date = None;
                self.state.oldest_loaded_timestamp = None;
                self.state.newest_loaded_timestamp = None;

                let _ = sender.output(ChatViewOutput::ChatClosed);
            }

            ChatViewInput::SendMessage => {
                // TODO: wire up actual message sending
            }
            ChatViewInput::MessageReceived(message) => {
                // If the bottom has been trimmed, skip appending — the message will
                // appear when the user scrolls back to bottom and triggers a reload.
                if self.state.bottom_trimmed {
                    return;
                }

                // Convert to local date for separator comparison.
                let msg_date = message.timestamp.with_timezone(&Local).date_naive();

                // Insert a date separator if the date changed.
                if self.state.last_message_date.map_or(true, |d| d != msg_date) {
                    self.list_view_wrapper
                        .append(ChatRow::DateSeparator(msg_date));
                    self.row_metadata
                        .push_back(RowMetadata::Separator(msg_date));
                    self.state.last_message_date = Some(msg_date);
                }

                // Update newest loaded timestamp to this message.
                let ts = message.timestamp.timestamp();
                self.state.newest_loaded_timestamp = Some(ts);

                self.list_view_wrapper.append(ChatRow::Message(message));
                self.row_metadata.push_back(RowMetadata::Message(ts));

                // If the user is at the bottom, they're seeing this message — mark read.
                if self.state.is_at_bottom {
                    if let Some(ref chat) = self.chat {
                        let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                    }
                }
            }

            ChatViewInput::PresenceUpdate {
                jid,
                available,
                last_seen,
            } => {
                if let Some(ref mut chat) = self.chat {
                    if jid == chat.jid {
                        if !chat.is_group() {
                            chat.available = Some(available);
                        }
                        chat.last_seen = last_seen;

                        // Update the user presence label.
                        self.update_presence();
                    }
                }
            }

            ChatViewInput::ScrollToBottom => {
                // If either end has been trimmed, the view is a "window" into the
                // message history — reload from scratch to jump to the real latest.
                if self.state.bottom_trimmed || self.state.top_trimmed {
                    self.row_metadata.clear();
                    self.list_view_wrapper.clear();
                    self.state.top_trimmed = false;
                    self.state.bottom_trimmed = false;
                    self.state.first_message_date = None;
                    self.state.last_message_date = None;

                    if let Some(ref chat) = self.chat {
                        if let Ok(messages) = chat.load_messages(INITIAL_LOAD_COUNT).await {
                            self.state.has_more_messages =
                                messages.len() as u32 == INITIAL_LOAD_COUNT;

                            // Track the oldest loaded timestamp for pagination.
                            if let Some(oldest) = messages.last() {
                                self.state.oldest_loaded_timestamp =
                                    Some(oldest.timestamp.timestamp());
                            }

                            // Track the newest loaded timestamp for downward pagination.
                            if let Some(newest) = messages.first() {
                                self.state.newest_loaded_timestamp =
                                    Some(newest.timestamp.timestamp());
                            }

                            for msg in messages.iter().rev() {
                                // Convert to local date for separator comparison.
                                let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

                                // Insert a date separator if the date changed.
                                if self.state.last_message_date.map_or(true, |d| d != msg_date) {
                                    self.list_view_wrapper
                                        .append(ChatRow::DateSeparator(msg_date));
                                    self.row_metadata
                                        .push_back(RowMetadata::Separator(msg_date));
                                    self.state.last_message_date = Some(msg_date);
                                }

                                // Track the first message date for prepend separators.
                                if self.state.first_message_date.is_none() {
                                    self.state.first_message_date = Some(msg_date);
                                }

                                self.list_view_wrapper.append(ChatRow::Message(msg.clone()));
                                self.row_metadata
                                    .push_back(RowMetadata::Message(msg.timestamp.timestamp()));
                            }
                        }
                    }
                }

                // Scroll to the last message.
                let count = self.list_view_wrapper.len();
                if count > 0 {
                    let info = gtk::ScrollInfo::new();
                    info.set_enable_vertical(true);
                    self.list_view_wrapper.view.scroll_to(
                        (count - 1) as u32,
                        gtk::ListScrollFlags::FOCUS,
                        Some(info),
                    );

                    self.state.is_at_bottom = true;
                }
            }
        }
    }

    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            ChatViewCommand::LoadOlderMessages => {
                // Guard against concurrent loads and exhausted history.
                if self.state.is_loading || !self.state.has_more_messages {
                    return;
                }

                let Some(ref chat) = self.chat else { return };
                let Some(before_ts) = self.state.oldest_loaded_timestamp else {
                    return;
                };

                self.state.is_loading = true;

                if let Ok(messages) = chat.load_messages_before(before_ts, LOAD_MORE_COUNT).await {
                    self.state.has_more_messages = messages.len() as u32 == LOAD_MORE_COUNT;

                    // Update the oldest loaded timestamp cursor.
                    if let Some(oldest) = messages.last() {
                        self.state.oldest_loaded_timestamp = Some(oldest.timestamp.timestamp());
                    }

                    // Reverse messages to get chronological order for prepending.
                    let mut insert_pos: u32 = 0;
                    let mut prev_date: Option<NaiveDate> = None;

                    for msg in messages.iter().rev() {
                        let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

                        // Insert a date separator if the date changed.
                        if prev_date.map_or(true, |d| d != msg_date) {
                            self.list_view_wrapper
                                .insert(insert_pos, ChatRow::DateSeparator(msg_date));
                            self.row_metadata
                                .insert(insert_pos as usize, RowMetadata::Separator(msg_date));
                            insert_pos += 1;
                            prev_date = Some(msg_date);
                        }

                        self.list_view_wrapper
                            .insert(insert_pos, ChatRow::Message(msg.clone()));
                        self.row_metadata.insert(
                            insert_pos as usize,
                            RowMetadata::Message(msg.timestamp.timestamp()),
                        );

                        insert_pos += 1;
                    }

                    // Remove duplicate date separator if the last prepended date matches
                    // the first existing date separator.
                    if let Some(last_prepended_date) = prev_date {
                        if Some(last_prepended_date) == self.state.first_message_date
                            && insert_pos < self.list_view_wrapper.len()
                        {
                            self.list_view_wrapper.remove(insert_pos);
                            self.row_metadata.remove(insert_pos as usize);
                        }
                    }

                    // Update first_message_date to the oldest prepended message's date.
                    if let Some(oldest_msg) = messages.last() {
                        self.state.first_message_date =
                            Some(oldest_msg.timestamp.with_timezone(&Local).date_naive());
                    }

                    // Trim excess rows from the bottom to stay within MAX_LOADED_ROWS.
                    let total = self.list_view_wrapper.len();
                    if total > MAX_LOADED_ROWS {
                        let to_remove = total - MAX_LOADED_ROWS;
                        for _ in 0..to_remove {
                            self.list_view_wrapper
                                .remove(self.list_view_wrapper.len() - 1);
                            self.row_metadata.pop_back();
                        }

                        self.state.bottom_trimmed = true;

                        // Update bottom cursors from remaining metadata.
                        self.update_bottom_cursors();
                    }
                }

                self.state.is_loading = false;
            }
            ChatViewCommand::LoadNewerMessages => {
                // Guard against concurrent loads and no trimmed tail to restore.
                if self.state.is_loading || !self.state.bottom_trimmed {
                    return;
                }

                let Some(ref chat) = self.chat else { return };
                let Some(after_ts) = self.state.newest_loaded_timestamp else {
                    return;
                };

                self.state.is_loading = true;

                if let Ok(messages) = chat.load_messages_after(after_ts, LOAD_MORE_COUNT).await {
                    // If fewer messages returned than requested, we've reached the real bottom.
                    if (messages.len() as u32) < LOAD_MORE_COUNT {
                        self.state.bottom_trimmed = false;
                    }

                    // Update the newest loaded timestamp cursor.
                    if let Some(newest) = messages.last() {
                        self.state.newest_loaded_timestamp = Some(newest.timestamp.timestamp());
                    }

                    for msg in messages.iter() {
                        let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

                        // Insert a date separator if the date changed.
                        if self.state.last_message_date.map_or(true, |d| d != msg_date) {
                            self.list_view_wrapper
                                .append(ChatRow::DateSeparator(msg_date));
                            self.row_metadata
                                .push_back(RowMetadata::Separator(msg_date));
                            self.state.last_message_date = Some(msg_date);
                        }

                        self.list_view_wrapper.append(ChatRow::Message(msg.clone()));
                        self.row_metadata
                            .push_back(RowMetadata::Message(msg.timestamp.timestamp()));
                    }

                    // Trim excess rows from the top to stay within MAX_LOADED_ROWS.
                    let total = self.list_view_wrapper.len();
                    if total > MAX_LOADED_ROWS {
                        let to_remove = total - MAX_LOADED_ROWS;
                        for _ in 0..to_remove {
                            self.list_view_wrapper.remove(0);
                            self.row_metadata.pop_front();
                        }

                        self.state.top_trimmed = true;

                        // Update top cursors from remaining metadata.
                        self.update_top_cursors();
                    }
                }

                self.state.is_loading = false;
            }

            ChatViewCommand::ScrollPositionChanged { at_top, at_bottom } => {
                if at_top && self.state.top_trimmed {
                    sender.oneshot_command(async { ChatViewCommand::LoadOlderMessages });
                } else if at_bottom && self.state.bottom_trimmed {
                    sender.oneshot_command(async { ChatViewCommand::LoadNewerMessages });
                }

                if at_bottom != self.state.is_at_bottom {
                    self.state.is_at_bottom = at_bottom;
                }
            }
        }
    }
}

impl ChatView {
    /// Update bottom cursors (`newest_loaded_timestamp`, `last_message_date`)
    /// from the row_metadata after trimming rows from the bottom.
    fn update_bottom_cursors(&mut self) {
        self.state.last_message_date = None;
        self.state.newest_loaded_timestamp = None;

        // Walk backward through metadata to find the newest message and last date.
        for meta in self.row_metadata.iter().rev() {
            match meta {
                RowMetadata::Message(ts) => {
                    if self.state.newest_loaded_timestamp.is_none() {
                        self.state.newest_loaded_timestamp = Some(*ts);
                    }
                }
                RowMetadata::Separator(date) => {
                    if self.state.last_message_date.is_none() {
                        self.state.last_message_date = Some(*date);
                    }
                }
            }

            // Stop once both cursors are found.
            if self.state.newest_loaded_timestamp.is_some()
                && self.state.last_message_date.is_some()
            {
                break;
            }
        }
    }

    /// Update top cursors (`oldest_loaded_timestamp`, `first_message_date`)
    /// from the row_metadata after trimming rows from the top.
    fn update_top_cursors(&mut self) {
        self.state.first_message_date = None;
        self.state.oldest_loaded_timestamp = None;

        // Walk forward through metadata to find the oldest message and first date.
        for meta in self.row_metadata.iter() {
            match meta {
                RowMetadata::Message(ts) => {
                    if self.state.oldest_loaded_timestamp.is_none() {
                        self.state.oldest_loaded_timestamp = Some(*ts);
                    }
                }
                RowMetadata::Separator(date) => {
                    if self.state.first_message_date.is_none() {
                        self.state.first_message_date = Some(*date);
                    }
                }
            }

            // Stop once both cursors are found.
            if self.state.oldest_loaded_timestamp.is_some()
                && self.state.first_message_date.is_some()
            {
                break;
            }
        }

        // We trimmed from top, so there are definitely older messages to load.
        self.state.has_more_messages = true;
    }
}

/// A single row in the chat history list.
#[derive(Clone, Debug)]
pub enum ChatRow {
    /// A regular chat message bubble.
    Message(ChatMessage),
    /// A date separator label (e.g. "Today", "Yesterday").
    DateSeparator(NaiveDate),
    /// A service/system event (e.g. "someone added xxx").
    ServiceEvent { text: String },
}

pub struct ChatRowWidgets {
    /// Outer container for message bubbles.
    message_box: gtk::Box,
    /// The message bubble itself.
    bubble_box: gtk::Box,
    /// Sender name label (visible in group chats for incoming messages).
    sender_label: gtk::Label,
    /// Message text content.
    content_label: gtk::Label,
    /// Timestamp label (e.g. "14:30").
    timestamp_label: gtk::Label,

    /// Date separator label (e.g. "Today", "Yesterday").
    separator_label: gtk::Label,
    /// Service event label (e.g. "someone added xxx").
    service_label: gtk::Label,
}

impl RelmListItem for ChatRow {
    type Root = gtk::Box;
    type Widgets = ChatRowWidgets;

    fn setup(_list_item: &gtk::ListItem) -> (Self::Root, Self::Widgets) {
        // Root container stacks all row variants vertically.
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        // Date separator (e.g. "Today").
        let separator_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .css_classes(["service-message", "caption", "dimmed"])
            .margin_top(12)
            .margin_bottom(4)
            .visible(false)
            .build();
        root.append(&separator_label);

        // Service event (e.g. "someone added xxx").
        let service_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .css_classes(["service-message", "caption", "dimmed"])
            .margin_top(4)
            .margin_bottom(4)
            .visible(false)
            .build();
        root.append(&service_label);

        // Message bubble container.
        let message_box = gtk::Box::builder()
            .visible(false)
            .spacing(0)
            .orientation(gtk::Orientation::Horizontal)
            .build();

        let bubble_box = gtk::Box::builder()
            .spacing(2)
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["message-bubble", "card"])
            .build();

        let sender_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .css_classes(["sender-name", "heading"])
            .visible(false)
            .build();
        bubble_box.append(&sender_label);

        let content_box = gtk::Box::builder()
            .spacing(12)
            .orientation(gtk::Orientation::Horizontal)
            .build();

        let content_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .xalign(0.0)
            .hexpand(true)
            .selectable(true)
            .css_classes(["body"])
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        content_box.append(&content_label);

        let timestamp_label = gtk::Label::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["caption", "dimmed", "numeric"])
            .build();
        content_box.append(&timestamp_label);

        bubble_box.append(&content_box);
        message_box.append(&bubble_box);
        root.append(&message_box);

        let widgets = ChatRowWidgets {
            message_box,
            bubble_box,
            sender_label,
            content_label,
            timestamp_label,
            separator_label,
            service_label,
        };

        (root, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, _root: &mut Self::Root) {
        // Hide all variants first, then show the active one.
        widgets.separator_label.set_visible(false);
        widgets.service_label.set_visible(false);
        widgets.message_box.set_visible(false);

        match self {
            Self::DateSeparator(date) => {
                widgets.separator_label.set_label(&format_date_label(*date));
                widgets.separator_label.set_visible(true);
                widgets.separator_label.set_focusable(false);
            }
            Self::ServiceEvent { text } => {
                widgets.service_label.set_label(text);
                widgets.service_label.set_visible(true);
                widgets.service_label.set_focusable(false);
            }
            Self::Message(msg) => {
                widgets.message_box.set_visible(true);
                widgets.message_box.set_focusable(false);
                widgets.content_label.set_label(&msg.content);
                widgets
                    .timestamp_label
                    .set_label(&msg.timestamp.format("%H:%M").to_string());

                widgets.bubble_box.remove_css_class("incoming");
                widgets.bubble_box.remove_css_class("outgoing");

                if msg.outgoing {
                    widgets.message_box.set_halign(gtk::Align::End);
                    widgets.bubble_box.add_css_class("outgoing");
                    widgets.bubble_box.set_margin_start(60);
                    widgets.bubble_box.set_margin_end(6);
                    widgets.sender_label.set_visible(false);
                } else {
                    widgets.message_box.set_halign(gtk::Align::Start);
                    widgets.bubble_box.add_css_class("incoming");
                    widgets.bubble_box.set_margin_start(6);
                    widgets.bubble_box.set_margin_end(60);

                    if msg.chat_jid.ends_with("@g.us") {
                        if let Some(ref name) = msg.sender_name {
                            widgets.sender_label.set_label(name);
                            widgets.sender_label.set_visible(true);
                        } else {
                            widgets.sender_label.set_visible(false);
                        }
                    } else {
                        widgets.sender_label.set_visible(false);
                    }
                }

                widgets.bubble_box.set_margin_top(2);
                widgets.bubble_box.set_margin_bottom(2);
            }
        }
    }
}
