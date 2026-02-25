use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use chrono::{Local, NaiveDate};
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

#[derive(Debug)]
pub struct ChatView {
    /// Whether the scroll is at the bottom.
    is_at_bottom: Rc<Cell<bool>>,
    /// `ListView` widget wrapper containing all chat rows.
    list_view_wrapper: TypedListView<ChatRow, gtk::NoSelection>,

    /// Currently open chat.
    chat: Option<Chat>,
    /// Date of the last appended message, used to insert date separators.
    last_message_date: Option<NaiveDate>,
}

#[derive(Debug)]
pub enum ChatViewInput {
    /// Open a chat.
    Open(Chat),

    /// New message received.
    MessageReceived(ChatMessage),
    /// Send a message.
    SendMessage,
}

#[derive(Debug)]
pub enum ChatViewOutput {
    /// The user is viewing this chat — mark it as read.
    MarkChatRead(String),
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for ChatView {
    type Init = ();
    type Input = ChatViewInput;
    type Output = ChatViewOutput;

    view! {
        adw::ToolbarView {
            set_css_classes: &["chat-view"],

            add_top_bar = &adw::HeaderBar {
                set_css_classes: &["flat"],
                #[watch]
                set_show_back_button: model.chat.is_some(),

                #[wrap(Some)]
                set_title_widget = &gtk::Button {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    set_css_classes: &["chat-title", "flat"], // Add `with-subtitle` when it have a description.

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
                            set_label: "A good description", // TODO: chat description or user status.
                            set_visible: false,
                            set_selectable: false,
                            set_css_classes: &["subtitle"],
                        },
                    },
                },
            },

            #[name = "scroll_window"]
            #[wrap(Some)]
            set_content = &gtk::ScrolledWindow {
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
                        set_single_click_activate: false,
                    }
                },
            },

            add_bottom_bar = &gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 6,
                set_margin_all: 6,

                #[name = "message_entry"]
                gtk::Entry {
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
        _sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let is_at_bottom = Rc::new(Cell::new(true));
        let list_view_wrapper = TypedListView::new();

        let model = Self {
            is_at_bottom,
            list_view_wrapper,

            chat: None,
            last_message_date: None,
        };

        let list_view = &model.list_view_wrapper.view;
        let widgets = view_output!();

        let adj = widgets.scroll_window.vadjustment();
        let flag = model.is_at_bottom.clone();
        adj.connect_value_changed(move |adj| {
            let at_bottom = adj.value() + adj.page_size() >= adj.upper() - 50.0;
            flag.set(at_bottom);
        });

        widgets.message_entry.grab_focus();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatViewInput::Open(chat) => {
                self.list_view_wrapper.clear();
                self.last_message_date = None;

                let jid = chat.jid.clone();

                // Load the last 100 messages.
                if let Ok(messages) = chat.load_messages(100).await {
                    for msg in messages.iter().rev() {
                        // Convert to local date for separator comparison.
                        let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

                        // Insert a date separator if the date changed.
                        if self.last_message_date.map_or(true, |d| d != msg_date) {
                            self.list_view_wrapper
                                .append(ChatRow::DateSeparator(msg_date));
                            self.last_message_date = Some(msg_date);
                        }

                        self.list_view_wrapper.append(ChatRow::Message(msg.clone()));
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
                    }
                }

                // Mark chat as read if it has unread messages.
                if chat.get_unread_count().await.is_ok_and(|count| count > 0) {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(jid));
                }

                self.chat = Some(chat);
            }

            ChatViewInput::MessageReceived(message) => {
                // Convert to local date for separator comparison.
                let msg_date = message.timestamp.with_timezone(&Local).date_naive();

                // Insert a date separator if the date changed.
                if self.last_message_date.map_or(true, |d| d != msg_date) {
                    self.list_view_wrapper
                        .append(ChatRow::DateSeparator(msg_date));
                    self.last_message_date = Some(msg_date);
                }

                self.list_view_wrapper.append(ChatRow::Message(message));

                // If the user is at the bottom, they're seeing this message — mark read.
                if self.is_at_bottom.get() {
                    if let Some(ref chat) = self.chat {
                        // Mark chat as read if it has unread messages.
                        if chat.get_unread_count().await.is_ok_and(|count| count > 0) {
                            let _ = sender.output(ChatViewOutput::MarkChatRead(chat.jid.clone()));
                        }
                    }
                }
            }

            ChatViewInput::SendMessage => {
                // TODO: wire up actual message sending
            }
        }
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
            .focusable(false)
            .css_classes(["service-message", "dimmed", "caption"])
            .margin_top(12)
            .margin_bottom(4)
            .visible(false)
            .build();
        root.append(&separator_label);

        // Service event (e.g. "someone added xxx").
        let service_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .focusable(false)
            .css_classes(["service-message", "dimmed", "caption"])
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
            }
            Self::ServiceEvent { text } => {
                widgets.service_label.set_label(text);
                widgets.service_label.set_visible(true);
            }
            Self::Message(msg) => {
                widgets.message_box.set_visible(true);
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
