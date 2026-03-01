use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use adw::prelude::*;
use chrono::Local;
use indexmap::IndexMap;
use relm4::{RelmListBoxExt, prelude::*};

use crate::{
    i18n,
    state::Chat,
    utils::{format_lid_as_number, get_first_name},
};

#[derive(Debug)]
pub struct ChatList {
    /// `ListBox` widget containing all chats.
    list_box: gtk::ListBox,
    /// Chat rows indexed by JID.
    chat_rows: IndexMap<String, adw::ActionRow>,
    /// Guard flag to suppress selection signals during list mutations.
    suppress_selection: Arc<AtomicBool>,

    /// Currently selected chat JID.
    chat_jid: Option<String>,
}

#[derive(Debug)]
pub enum ChatListInput {
    /// Add a chat.
    AddChat {
        chat: Chat,
        /// Whether add the chat in the top of the list.
        at_top: bool,
    },
    /// Select a chat.
    Select(String),
    /// Update a chat in place.
    UpdateChat {
        chat: Chat,
        /// Whether move the chat to the top of the list.
        move_to_top: bool,
    },
    /// Select a chat by index.
    SelectIndex(usize),
    /// Clear the chat selection.
    ClearSelection,
}

#[derive(Debug)]
pub enum ChatListOutput {
    /// A chat has been selected.
    ChatSelected(String),
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
            set_hscrollbar_policy: gtk::PolicyType::Never,
            set_overlay_scrolling: true,
            set_propagate_natural_width: true,

            #[local_ref]
            list_box -> gtk::ListBox {
                set_css_classes: &["navigation-sidebar"],
                set_selection_mode: gtk::SelectionMode::Single,

                connect_row_selected[sender, suppress = model.suppress_selection.clone()] => move |_, row| {
                    if suppress.load(Ordering::Acquire) {
                        return;
                    }

                    if let Some(row) = row {
                        let jid = row.widget_name();
                        if !jid.is_empty() {
                            sender.input(ChatListInput::Select(jid.into()));
                        }
                    }
                },
            }
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let list_box = gtk::ListBox::new();
        let suppress_selection = Arc::new(AtomicBool::new(false));

        let model = Self {
            list_box,
            chat_rows: IndexMap::new(),
            suppress_selection,

            chat_jid: None,
        };

        let list_box = &model.list_box;
        let widgets = view_output!();

        model.list_box.unselect_all();

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatListInput::AddChat { chat, at_top } => {
                let row = build_chat_row(&chat).await;

                // Insert at the top if specified.
                if at_top {
                    self.list_box.prepend(&row);
                } else {
                    self.list_box.append(&row);
                }

                // Insert in our widget tree.
                self.chat_rows.insert(chat.jid.clone(), row);
            }
            ChatListInput::Select(jid) => {
                // Check if the selected chat isn't already selected.
                if self.chat_jid.as_deref() != Some(&jid) {
                    self.chat_jid = Some(jid.clone());
                    let _ = sender.output(ChatListOutput::ChatSelected(jid));
                }
            }
            ChatListInput::UpdateChat { chat, move_to_top } => {
                self.suppress_selection.store(true, Ordering::Release);

                // Replace the row widget in place.
                if let Some(old_row) = self.chat_rows.shift_remove(&chat.jid) {
                    // Get the index of the row.
                    let index = self
                        .list_box
                        .index_of_child(&old_row)
                        .unwrap_or_else(|| old_row.index());
                    // Remove the row from the list.
                    self.list_box.remove(&old_row);

                    // Build the updated row.
                    let row = build_chat_row(&chat).await;

                    // Insert at the top if specified.
                    if move_to_top {
                        self.list_box.prepend(&row);
                    } else {
                        self.list_box.insert(&row, index);
                    }

                    // Re-select if this row was the active chat.
                    if self.chat_jid.as_deref() == Some(chat.jid.as_str()) {
                        self.list_box
                            .select_row(Some(row.upcast_ref::<gtk::ListBoxRow>()));
                    }

                    // Re-insert in our widget tree.
                    self.chat_rows.insert(chat.jid.clone(), row);
                }

                self.suppress_selection.store(false, Ordering::Release);
            }
            ChatListInput::SelectIndex(index) => {
                if let Some((key, _)) = self.chat_rows.get_index(index) {
                    // Check if the selected chat isn't already selected.
                    if self.chat_jid.as_deref() != Some(key) {
                        sender.input(ChatListInput::Select(key.clone()));
                    }
                }
            }
            ChatListInput::ClearSelection => {
                self.chat_jid = None;
                self.list_box.unselect_all();
            }
        }
    }
}

/// Build a new chat row widget for the given chat.
#[allow(clippy::too_many_lines)]
async fn build_chat_row(chat: &Chat) -> adw::ActionRow {
    let avatar = {
        let overlay = gtk::Overlay::new();

        let avatar = adw::Avatar::builder()
            .size(36)
            .text(&chat.name)
            .show_initials(true)
            .build();
        overlay.set_child(Some(&avatar));

        // TODO: online dot

        overlay
    };

    let row = {
        let name = if chat.name.trim().is_empty() {
            format_lid_as_number(&chat.jid)
        } else {
            chat.name.trim().to_string()
        };
        let mut builder = adw::ActionRow::builder()
            .name(&chat.jid)
            .title(&name)
            .title_lines(1)
            .use_markup(false)
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

        let unread_count = chat.get_unread_count().await.unwrap_or(0);
        if unread_count > 0 {
            let badge = gtk::Label::builder()
                .label(unread_count.to_string())
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
