use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use relm4::{
    prelude::*,
    typed_view::list::{RelmListItem, TypedListView},
};

use crate::state::{Chat, ChatMessage};

#[derive(Debug)]
pub struct ChatView {
    /// Whether the scroll is at the bottom.
    is_at_bottom: Rc<Cell<bool>>,
    /// `ListView` widget wrapper containing all chat messages.
    list_view_wrapper: TypedListView<ChatMessage, gtk::NoSelection>,

    /// Currently open chat.
    chat: Option<Chat>,
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
    /// Mark the open chat as read.
    MarkChatRead(String),
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for ChatView {
    type Init = ();
    type Input = ChatViewInput;
    type Output = ChatViewOutput;

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                set_css_classes: &["flat"],
                #[watch]
                set_show_back_button: model.chat.is_some(),

                #[wrap(Some)]
                set_title_widget = &gtk::Button {
                    set_halign: gtk::Align::Center,
                    set_valign: gtk::Align::Center,
                    set_css_classes: &["flat"],

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
                    },
                },
            },

            #[name = "scroll_window"]
            #[wrap(Some)]
            set_content = &gtk::ScrolledWindow {
                set_css_classes: &["undershoot-bottom"],
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
                    set_placeholder_text: Some("Message"),

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

                let jid = chat.jid.clone();

                // Load the last 100 messages.
                if let Ok(messages) = chat.load_messages(100).await {
                    for msg in messages.iter().rev() {
                        self.list_view_wrapper.append(msg.clone());
                    }

                    // Scroll to the last message.
                    let info = gtk::ScrollInfo::new();
                    info.set_enable_vertical(true);
                    self.list_view_wrapper.view.scroll_to(
                        (messages.len() - 1) as u32,
                        gtk::ListScrollFlags::FOCUS,
                        Some(info),
                    );
                }

                // Mark chat as read if it has unread messages.
                if chat.get_unread_count().await.is_ok_and(|count| count > 0) {
                    let _ = sender.output(ChatViewOutput::MarkChatRead(jid));
                }

                self.chat = Some(chat);
            }

            ChatViewInput::MessageReceived(message) => {
                self.list_view_wrapper.append(message);

                // If the user is at the bottom, they're seeing this message → mark read.
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

pub struct MessageWidgets {
    outer_box: gtk::Box,
    bubble_box: gtk::Box,
    sender_label: gtk::Label,
    content_label: gtk::Label,
    timestamp_label: gtk::Label,
}

impl RelmListItem for ChatMessage {
    type Root = gtk::Box;
    type Widgets = MessageWidgets;

    fn setup(_list_item: &gtk::ListItem) -> (Self::Root, Self::Widgets) {
        let outer_box = gtk::Box::builder()
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
            .visible(false)
            .css_classes(["sender-name", "heading"])
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
            .selectable(true)
            .css_classes(["body"])
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        content_box.append(&content_label);

        let timestamp_label = gtk::Label::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["caption", "dimmed", "numeric"])
            .build();
        content_box.append(&timestamp_label);

        bubble_box.append(&content_box);
        outer_box.append(&bubble_box);

        let widgets = MessageWidgets {
            outer_box: outer_box.clone(),
            bubble_box,
            sender_label,
            content_label,
            timestamp_label,
        };

        (outer_box, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, _root: &mut Self::Root) {
        widgets.content_label.set_label(&self.content);
        widgets
            .timestamp_label
            .set_label(&self.timestamp.format("%H:%M").to_string());

        widgets.bubble_box.remove_css_class("incoming");
        widgets.bubble_box.remove_css_class("outgoing");

        if self.outgoing {
            widgets.outer_box.set_halign(gtk::Align::End);
            widgets.bubble_box.add_css_class("outgoing");
            widgets.bubble_box.set_margin_start(60);
            widgets.bubble_box.set_margin_end(6);
            widgets.sender_label.set_visible(false);
        } else {
            widgets.outer_box.set_halign(gtk::Align::Start);
            widgets.bubble_box.add_css_class("incoming");
            widgets.bubble_box.set_margin_start(6);
            widgets.bubble_box.set_margin_end(60);

            let is_group = self.chat_jid.ends_with("@g.us");
            if is_group {
                if let Some(ref name) = self.sender_name {
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
