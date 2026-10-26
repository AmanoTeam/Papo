use adw::prelude::*;
use gtk::{gdk, glib, pango};
use jiff::{civil::Date, tz::TimeZone};
use relm4::{RelmWidgetExt, gtk, typed_view::list::RelmListItem};

use crate::{
    components::ChatViewInput,
    i18n,
    session::avatar,
    state::{ChatMessage, MessageStatus},
    utils::{bare_jid, format_date_label, is_group_jid},
    widgets::{MessageTail, TailDirection},
};

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ChatRow {
    Message {
        last: bool,
        first: bool,
        unread: bool,
        message: ChatMessage,
        last_of_segment: bool,
        first_of_segment: bool,
    },
    DateSeparator(Date),
    ServiceEvent {
        text: String,
    },
    UnreadDivider,
}

pub struct ChatRowWidgets {
    input: Option<relm4::Sender<ChatViewInput>>,
    avatar: adw::Avatar,
    tail_left: MessageTail,
    bubble_box: gtk::Box,
    tail_right: MessageTail,
    avatar_slot: gtk::Box,
    message_box: gtk::Box,
    status_icon: gtk::Image,
    sender_label: gtk::Label,
    quote_box: gtk::Box,
    quote_sender: gtk::Label,
    quote_body: gtk::Label,
    content_label: gtk::Label,
    divider_label: gtk::Label,
    service_label: gtk::Label,
    separator_label: gtk::Label,
    timestamp_label: gtk::Label,
    divider_separator: gtk::Separator,
}

impl ChatRowWidgets {
    pub(crate) fn set_input(&mut self, input: relm4::Sender<ChatViewInput>) {
        self.input = Some(input.clone());

        let click = gtk::GestureClick::new();
        let quote_box = self.quote_box.clone();
        click.connect_pressed(move |_, _, _, _| {
            let stanza = quote_box.widget_name();
            if !stanza.is_empty() {
                let _ = input.send(ChatViewInput::QuoteActivated {
                    stanza_id: stanza.to_string(),
                });
            }
        });
        self.quote_box.add_controller(click);
    }
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
        // Wire push names carry the `~` marker; initials ignore it.
        let name = msg
            .sender_name
            .clone()
            .map(|name| name.trim_start_matches('~').to_string());
        widgets.avatar.set_text(name.as_deref());
        widgets.avatar.set_custom_image(None::<&gdk::Texture>);

        // Lazy per-sender pictures: attach from the decode cache, decode
        // in the background, or ask the component to fetch the file.
        let sender = bare_jid(&msg.sender_jid);
        if !sender.is_empty() && !is_group_jid(&sender) {
            widgets.avatar.set_widget_name(&sender);

            match avatar::sender_cached_path(&sender) {
                Some(path) => {
                    if let Some(texture) = avatar::texture(&path) {
                        widgets.avatar.set_custom_image(Some(&texture));
                    } else {
                        let key = sender.clone();
                        let avatar = widgets.avatar.clone();

                        glib::spawn_future_local(async move {
                            if let Some(texture) = avatar::load(&path).await
                                && avatar.widget_name() == key
                            {
                                avatar.set_custom_image(Some(&texture));
                            }
                        });
                    }
                }
                None => {
                    if let Some(input) = &widgets.input {
                        let _ =
                            input.send(super::ChatViewInput::RequestSenderAvatar { jid: sender });
                    }
                }
            }
        } else {
            widgets.avatar.set_widget_name("");
        }
    }

    if first && let Some(name) = &msg.sender_name {
        if name.starts_with('~') {
            widgets.sender_label.add_css_class("push-name");
        } else {
            widgets.sender_label.remove_css_class("push-name");
        }

        widgets.sender_label.set_label(name);
        widgets.sender_label.set_visible(true);
    }
}

fn bind_quote(widgets: &ChatRowWidgets, msg: &ChatMessage, sender_visible: bool) {
    let Some(quote) = &msg.quote else {
        widgets.quote_box.set_widget_name("");
        return;
    };

    widgets.quote_box.set_visible(true);
    widgets
        .quote_box
        .set_margin_top(i32::from(sender_visible) * 3);
    widgets
        .quote_box
        .set_widget_name(quote.stanza_id.as_deref().unwrap_or_default());

    if msg.outgoing {
        widgets.quote_box.add_css_class("outgoing");
        widgets.quote_box.remove_css_class("incoming");
    } else {
        widgets.quote_box.remove_css_class("outgoing");
        widgets.quote_box.add_css_class("incoming");
    }

    if let Some(name) = quote.sender_name.as_deref().filter(|name| !name.is_empty()) {
        widgets.quote_sender.set_label(name.trim_start_matches('~'));
        widgets.quote_sender.set_visible(true);
    } else if let Some(participant) = &quote.participant {
        widgets.quote_sender.set_label(participant);
        widgets.quote_sender.set_visible(true);
    } else {
        widgets.quote_sender.set_visible(false);
    }

    widgets
        .quote_body
        .set_label(quote.text().unwrap_or_default());
}

fn bind_content_label(widgets: &ChatRowWidgets, msg: &ChatMessage) {
    let Some(media) = &msg.media else {
        widgets.content_label.set_label(&msg.content);
        return;
    };

    let label = media.r#type.display_label();
    widgets.content_label.set_label(&if msg.content.is_empty() {
        label
    } else {
        format!("{label}\n{}", msg.content)
    });
}

fn bind_message_side(widgets: &ChatRowWidgets, msg: &ChatMessage, first: bool, last: bool) {
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
        widgets.tail_right.set_opacity(if last { 1.0 } else { 0.0 });
    } else {
        widgets.message_box.set_halign(gtk::Align::Start);
        widgets.message_box.set_margin_start(6);
        widgets.message_box.set_margin_end(60);
        widgets.bubble_box.add_css_class("incoming");
        widgets.bubble_box.set_margin_start(0);
        widgets.bubble_box.set_margin_end(0);

        if is_group_jid(&msg.chat_jid) {
            bind_group_avatar(widgets, msg, first, last);
        }

        widgets.status_icon.set_visible(false);

        widgets.tail_left.set_visible(true);
        widgets.tail_left.set_opacity(if last { 1.0 } else { 0.0 });
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
            .css_classes(["message-row"])
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
            .margin_top(10)
            .margin_bottom(4)
            .build();
        root.append(&divider_label);

        let divider_separator = gtk::Separator::builder()
            .hexpand(true)
            .css_classes(["unread-divider-separator"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(4)
            .margin_bottom(4)
            .visible(false)
            .build();
        root.append(&divider_separator);

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
            .ellipsize(pango::EllipsizeMode::End)
            .visible(false)
            .build();
        bubble_box.append(&sender_label);

        let quote_box = gtk::Box::builder()
            .spacing(0)
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["quote-box"])
            .visible(false)
            .build();
        bubble_box.append(&quote_box);

        let quote_bar = gtk::Box::builder()
            .width_request(3)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Fill)
            .css_classes(["quote-bar"])
            .build();
        quote_box.append(&quote_bar);

        let quote_stack = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .spacing(1)
            .orientation(gtk::Orientation::Vertical)
            .margin_start(6)
            .margin_end(4)
            .build();
        quote_box.append(&quote_stack);

        let quote_sender = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .css_classes(["quote-sender"])
            .ellipsize(pango::EllipsizeMode::End)
            .visible(false)
            .build();
        quote_stack.append(&quote_sender);
        let quote_body = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .css_classes(["quote-text"])
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(32)
            .build();
        quote_stack.append(&quote_body);

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
            .width_chars(1)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .natural_wrap_mode(gtk::NaturalWrapMode::Word)
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
            input: None,
            avatar,
            tail_left,
            bubble_box,
            tail_right,
            avatar_slot,
            message_box,
            status_icon,
            sender_label,
            quote_box,
            quote_sender,
            quote_body,
            content_label,
            divider_label,
            service_label,
            separator_label,
            timestamp_label,
            divider_separator,
        };

        (root, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, root: &mut Self::Root) {
        // Hide all variants first, then show the active one.
        widgets.separator_label.set_visible(false);
        widgets.divider_label.set_visible(false);
        widgets.divider_separator.set_visible(false);
        widgets.service_label.set_visible(false);
        widgets.message_box.set_visible(false);
        widgets.quote_box.set_visible(false);

        root.remove_css_class("highlight");
        root.remove_css_class("unread-row");
        root.remove_css_class("unread-first");
        root.remove_css_class("unread-last");
        root.set_margin_top(0);

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
                widgets.divider_separator.set_visible(true);

                root.add_css_class("unread-row");
                root.add_css_class("unread-first");

                root.set_margin_top(8);
            }
            Self::ServiceEvent { text } => {
                widgets.service_label.set_label(text);
                widgets.service_label.set_visible(true);
                widgets.service_label.set_focusable(false);
            }
            Self::Message {
                last,
                first,
                unread,
                message: msg,
                last_of_segment,
                first_of_segment,
            } => {
                widgets.message_box.set_visible(true);
                widgets.message_box.set_focusable(false);

                bind_quote(
                    widgets,
                    msg,
                    *first && msg.sender_name.is_some() && !msg.outgoing,
                );
                bind_content_label(widgets, msg);

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

                if *unread {
                    root.add_css_class("unread-row");
                }
                if *first_of_segment {
                    root.add_css_class("unread-first");
                }
                if *last_of_segment {
                    root.add_css_class("unread-last");
                }
                bind_message_side(widgets, msg, *first, *last);

                widgets
                    .bubble_box
                    .set_margin_top(if *first { 4 } else { 1 });
                widgets.bubble_box.set_margin_bottom(2);
            }
        }
    }
}
