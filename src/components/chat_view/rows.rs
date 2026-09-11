use adw::prelude::*;
use jiff::{civil::Date, tz::TimeZone};
use relm4::{RelmWidgetExt, gtk, gtk::pango, typed_view::list::RelmListItem};

use crate::{
    i18n,
    state::{ChatMessage, MessageStatus},
    utils::format_date_label,
    widgets::{MessageTail, TailDirection},
};

/// A single row in the chat history list.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ChatRow {
    /// A regular chat message bubble.
    Message {
        last: bool,
        first: bool,
        message: ChatMessage,
    },
    /// A date separator label (e.g. "Today", "Yesterday").
    DateSeparator(Date),
    /// A service/system event (e.g. "someone added xxx").
    ServiceEvent { text: String },
    /// The unread messages divider.
    UnreadDivider,
}

pub struct ChatRowWidgets {
    avatar: adw::Avatar,
    tail_left: MessageTail,
    /// The message bubble itself.
    bubble_box: gtk::Box,
    tail_right: MessageTail,
    avatar_slot: gtk::Box,
    /// Outer container for message bubbles.
    message_box: gtk::Box,
    /// Message status icon (e.g. "Sending", "Sent").
    status_icon: gtk::Image,
    /// Sender name label (visible in group chats for incoming messages).
    sender_label: gtk::Label,
    /// Message text content.
    content_label: gtk::Label,
    /// Unread messages divider label.
    divider_label: gtk::Label,
    /// Service event label (e.g. "someone added xxx").
    service_label: gtk::Label,
    /// Date separator label (e.g. "Today", "Yesterday").
    separator_label: gtk::Label,
    /// Timestamp label (e.g. "14:30").
    timestamp_label: gtk::Label,
}

fn new_tail(direction: TailDirection, css_class: &str) -> MessageTail {
    let tail = MessageTail::new();
    tail.set_direction(direction);
    tail.set_css_classes(&["message-tail", css_class]);
    tail.set_valign(gtk::Align::End);
    tail.set_margin_bottom(2);
    tail.set_width_request(8);
    tail.set_height_request(13);

    tail
}

fn bind_group_avatar(widgets: &ChatRowWidgets, msg: &ChatMessage, first: bool, last: bool) {
    widgets.avatar_slot.set_visible(true);
    widgets.avatar.set_visible(last);

    if last {
        let name = msg
            .sender_name
            .clone()
            .or_else(|| msg.sender_jid.split('@').next().map(str::to_string));
        widgets.avatar.set_text(name.as_deref());
    }

    if first && let Some(name) = &msg.sender_name {
        widgets.sender_label.set_label(name);
        widgets.sender_label.set_visible(true);
    }
}

impl RelmListItem for ChatRow {
    type Root = gtk::Box;
    type Widgets = ChatRowWidgets;

    #[allow(clippy::too_many_lines)]
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
            .build();
        root.append(&separator_label);

        // Unread messages divider.
        let divider_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .css_classes(["unread-divider", "caption"])
            .margin_top(6)
            .margin_bottom(2)
            .build();
        root.append(&divider_label);

        // Service event (e.g. "someone added xxx").
        let service_label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .css_classes(["service-message", "caption", "dimmed"])
            .margin_top(4)
            .margin_bottom(4)
            .build();
        root.append(&service_label);

        // Message bubble container.
        let message_box = gtk::Box::builder()
            .spacing(0)
            .orientation(gtk::Orientation::Horizontal)
            .build();

        let avatar = adw::Avatar::builder()
            .size(32)
            .show_initials(true)
            .valign(gtk::Align::End)
            .margin_bottom(2)
            .build();

        let avatar_slot = gtk::Box::builder().width_request(32).build();
        avatar_slot.append(&avatar);

        let tail_left = new_tail(TailDirection::Left, "incoming");
        let tail_right = new_tail(TailDirection::Right, "outgoing");

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

        // Time and status container.
        let time_status_box = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .spacing(4)
            .orientation(gtk::Orientation::Horizontal)
            .build();

        let timestamp_label = gtk::Label::builder()
            .css_classes(["dimmed", "caption", "numeric"])
            .build();
        time_status_box.append(&timestamp_label);

        let status_icon = gtk::Image::builder()
            .pixel_size(12)
            .css_classes(["dimmed", "status-icon"])
            .build();
        time_status_box.append(&status_icon);

        content_box.append(&time_status_box);
        bubble_box.append(&content_box);
        message_box.append(&avatar_slot);
        message_box.append(&tail_left);
        message_box.append(&bubble_box);
        message_box.append(&tail_right);
        root.append(&message_box);

        let widgets = ChatRowWidgets {
            avatar,
            tail_left,
            bubble_box,
            tail_right,
            avatar_slot,
            message_box,
            status_icon,
            sender_label,
            content_label,
            divider_label,
            service_label,
            separator_label,
            timestamp_label,
        };

        (root, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, _root: &mut Self::Root) {
        // Hide all variants first, then show the active one.
        widgets.separator_label.set_visible(false);
        widgets.divider_label.set_visible(false);
        widgets.service_label.set_visible(false);
        widgets.message_box.set_visible(false);

        match self {
            Self::DateSeparator(date) => {
                widgets.separator_label.set_label(&format_date_label(*date));
                widgets.separator_label.set_visible(true);
                widgets.separator_label.set_focusable(false);
            }
            Self::UnreadDivider => {
                widgets.divider_label.set_label(&i18n!("Unread messages"));
                widgets.divider_label.set_visible(true);
                widgets.divider_label.set_focusable(false);
            }
            Self::ServiceEvent { text } => {
                widgets.service_label.set_label(text);
                widgets.service_label.set_visible(true);
                widgets.service_label.set_focusable(false);
            }
            Self::Message {
                last,
                first,
                message: msg,
            } => {
                widgets.message_box.set_visible(true);
                widgets.message_box.set_focusable(false);
                widgets.content_label.set_label(&msg.content);
                // Convert UTC timestamp to local time for display
                let local_time = msg.timestamp.to_zoned(TimeZone::system());
                widgets
                    .timestamp_label
                    .set_label(&local_time.strftime("%H:%M").to_string());

                widgets.bubble_box.remove_css_class("incoming");
                widgets.bubble_box.remove_css_class("outgoing");
                widgets.bubble_box.remove_css_class("group-first");
                widgets.bubble_box.remove_css_class("group-last");

                widgets.status_icon.set_has_tooltip(false);
                widgets.status_icon.remove_css_class("white");
                widgets.status_icon.remove_css_class("warning");

                widgets.tail_left.set_visible(false);
                widgets.tail_right.set_visible(false);
                widgets.avatar_slot.set_visible(false);
                widgets.avatar.set_visible(false);
                widgets.sender_label.set_visible(false);

                if *first {
                    widgets.bubble_box.add_css_class("group-first");
                }
                if *last {
                    widgets.bubble_box.add_css_class("group-last");
                }

                if msg.outgoing {
                    widgets.message_box.set_halign(gtk::Align::End);
                    widgets.message_box.set_margin_start(60);
                    widgets.message_box.set_margin_end(6);
                    widgets.bubble_box.add_css_class("outgoing");
                    widgets.bubble_box.set_margin_start(0);
                    widgets.bubble_box.set_margin_end(0);

                    widgets.status_icon.set_visible(true);
                    widgets
                        .status_icon
                        .set_icon_name(Some(msg.status.icon_name()));
                    match msg.status {
                        MessageStatus::Read => {
                            widgets.status_icon.add_css_class("white");
                        }
                        MessageStatus::Failed => {
                            widgets
                                .status_icon
                                .set_tooltip(&i18n!("The message could not be sent."));
                            widgets.status_icon.add_css_class("warning");
                        }
                        _ => {}
                    }

                    widgets.tail_right.set_visible(true);
                    widgets
                        .tail_right
                        .set_opacity(if *last { 1.0 } else { 0.0 });
                } else {
                    widgets.message_box.set_halign(gtk::Align::Start);
                    widgets.message_box.set_margin_start(6);
                    widgets.message_box.set_margin_end(60);
                    widgets.bubble_box.add_css_class("incoming");
                    widgets.bubble_box.set_margin_start(0);
                    widgets.bubble_box.set_margin_end(0);

                    if msg.chat_jid.ends_with("@g.us") {
                        bind_group_avatar(widgets, msg, *first, *last);
                    }

                    widgets.status_icon.set_visible(false);

                    widgets.tail_left.set_visible(true);
                    widgets.tail_left.set_opacity(if *last { 1.0 } else { 0.0 });
                }

                widgets
                    .bubble_box
                    .set_margin_top(if *first { 8 } else { 1 });
                widgets.bubble_box.set_margin_bottom(2);
            }
        }
    }
}
