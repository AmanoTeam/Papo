use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use chrono::Local;
use indexmap::IndexMap;
use relm4::prelude::*;

use crate::{
    i18n,
    state::Chat,
    utils::{format_lid_as_number, get_first_name},
};

pub struct ChatList {
    /// Cached chat list.
    chats: Vec<Chat>,
    /// `ListBox` widget containing all chats.
    list_box: gtk::ListBox,
    /// Chat rows.
    chat_rows: Rc<RefCell<IndexMap<String, adw::ActionRow>>>,
    /// Currently selected chat JID.
    selected_chat_jid: Option<String>,
}

#[derive(Debug)]
pub enum ChatListInput {
    /// Update chat list.
    Update { chats: Vec<Chat> },
    /// Select a chat.
    Select(String),
    /// Clear the chat selection.
    ClearSelection,
}

#[derive(Debug)]
pub enum ChatListOutput {
    /// A chat has been selected.
    ChatSelected(String),
}

impl ChatList {
    /// Rebuild the chat list.
    async fn rebuild_list(&self) {
        // Clear existing rows.
        self.list_box.remove_all();
        self.chat_rows.borrow_mut().clear();

        // Rebuild.
        for chat in &self.chats {
            let row = self.build_chat_row(chat).await;

            self.list_box.append(&row);
            self.chat_rows.borrow_mut().insert(chat.jid.clone(), row);
        }
    }

    /// Build a new chat row.
    #[allow(clippy::too_many_lines)]
    async fn build_chat_row(&self, chat: &Chat) -> adw::ActionRow {
        let avatar = {
            let overlay = gtk::Overlay::new();

            let name = if chat.name.trim().is_empty() {
                format_lid_as_number(&chat.jid)
            } else {
                chat.name.trim().to_string()
            };
            let avatar = adw::Avatar::builder()
                .size(36)
                .text(name)
                .show_initials(true)
                .build();
            overlay.set_child(Some(&avatar));

            // TODO: online dot

            overlay
        };

        let row = {
            let mut builder = adw::ActionRow::builder()
                .title(&chat.name)
                .title_lines(1)
                .activatable(true);

            if let Ok(Some(last_message)) = chat.get_last_message().await {
                let mut content = last_message.content;
                let mut first_line = if content.contains('\n') {
                    content
                        .split_once('\n')
                        .map(|(f, s)| {
                            if s.is_empty() {
                                f.to_string()
                            } else {
                                f.to_string() + "..."
                            }
                        })
                        .unwrap_or_default()
                } else {
                    content.clone()
                };

                if let Some(ref name) = last_message.sender_name {
                    if chat.is_group() && !last_message.outgoing {
                        content = format!("{name}: {content}");
                        first_line = format!("{}: {first_line}", get_first_name(name));
                    } else if last_message.outgoing {
                        content = format!("{}: {content}", i18n!("You"));
                        first_line = format!("{}: {first_line}", i18n!("You"));
                    }
                }

                builder = builder
                    .subtitle(first_line)
                    .subtitle_lines(1)
                    .tooltip_text(&content);
            } else {
                builder = builder.tooltip_text(&chat.name);
            }

            builder.build()
        };

        if chat.muted {
            row.add_css_class("dimmed");
        }

        row.add_prefix(&avatar);

        let suffix_box = {
            let suffix = gtk::Box::builder()
                .valign(gtk::Align::Center)
                .spacing(2)
                .orientation(gtk::Orientation::Vertical)
                .build();

            let top = gtk::Box::builder()
                .halign(gtk::Align::End)
                .spacing(4)
                .orientation(gtk::Orientation::Horizontal)
                .build();

            let bottom = gtk::Box::builder()
                .halign(gtk::Align::End)
                .spacing(4)
                .orientation(gtk::Orientation::Horizontal)
                .build();

            if let Ok(Some(last_message)) = chat.get_last_message().await {
                let now = Local::now();
                let timestamp = last_message.timestamp.with_timezone(&Local);
                let diff = now - timestamp;

                let sent_today = diff.num_days() == 0;

                let time_label = gtk::Label::builder()
                    .label(if sent_today {
                        timestamp.format("%H:%M").to_string()
                    } else {
                        timestamp.format("%d/%m").to_string()
                    })
                    .css_classes(["dimmed", "caption", "numeric"])
                    .build();
                top.append(&time_label);
            }

            if chat.muted {
                let icon = gtk::Image::builder()
                    .halign(gtk::Align::End)
                    .icon_name("speaker-0-symbolic")
                    .pixel_size(12)
                    .css_classes(["dimmed"])
                    .build();
                bottom.append(&icon);
            }

            if chat.pinned {
                let icon = gtk::Image::builder()
                    .halign(gtk::Align::End)
                    .icon_name("pin-symbolic")
                    .pixel_size(12)
                    .css_classes(["dimmed"])
                    .build();
                bottom.append(&icon);
            }

            if chat.unread_count > 0 {
                let badge = gtk::Label::builder()
                    .label(chat.unread_count.to_string())
                    .justify(gtk::Justification::Center)
                    .css_classes(if chat.muted {
                        vec!["badge", "muted", "numeric"]
                    } else {
                        vec!["badge", "numeric"]
                    })
                    .build();
                bottom.append(&badge);
            }

            suffix.append(&top);
            suffix.append(&bottom);

            suffix
        };

        row.add_suffix(&suffix_box);

        row
    }
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for ChatList {
    type Init = ();
    type Input = ChatListInput;
    type Output = ChatListOutput;

    view! {
        gtk::ScrolledWindow {
            set_vexpand: true,
            set_hexpand: true,
            set_css_classes: &["undershoot-tpp", "undershoot-bottom"],
            set_hscrollbar_policy: gtk::PolicyType::Never,
            set_overlay_scrolling: true,
            set_propagate_natural_width: true,

            #[local_ref]
            list_box -> gtk::ListBox {
                set_css_classes: &["navigation-sidebar"],
                set_selection_mode: gtk::SelectionMode::Single
            }
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let list_box = gtk::ListBox::new();
        let chat_rows = Rc::new(RefCell::new(IndexMap::new()));

        let model = Self {
            chats: Vec::new(),
            list_box,
            chat_rows,
            selected_chat_jid: None,
        };

        let list_box = &model.list_box;
        let widgets = view_output!();

        model.list_box.unselect_all();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatListInput::Update { chats } => {
                self.chats = chats;

                // Rebuild chat list.
                self.rebuild_list().await;
            }
            ChatListInput::Select(jid) => {
                self.selected_chat_jid = Some(jid.clone());
                let _ = sender.output(ChatListOutput::ChatSelected(jid));
            }
            ChatListInput::ClearSelection => {
                self.list_box.unselect_all();
                self.selected_chat_jid = None;
            }
        }
    }
}
