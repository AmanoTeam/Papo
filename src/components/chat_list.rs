use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use adw::prelude::*;
use chrono::Local;
use gtk::{gdk::Texture, gio, glib, pango};
use relm4::{
    prelude::*,
    typed_view::list::{RelmListItem, TypedListView},
};

use crate::{
    i18n,
    state::{Chat, ChatMessage},
    utils::{format_lid_as_number, get_first_name},
};

#[derive(Debug)]
pub struct ChatList {
    /// Current chat list state.
    state: ChatListState,
    /// Currently selected chat JID.
    chat_jid: Option<String>,
    /// `ListView` widget wrapper containing all chat rows.
    list_view_wrapper: TypedListView<ChatRow, gtk::SingleSelection>,
}

#[derive(Debug, Default)]
pub struct ChatListState {
    /// Whether the user is searching chats.
    searching_chats: Arc<AtomicBool>,
    /// Guard flag to suppress selection signals during list mutations.
    suppress_selection: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum ChatListInput {
    /// Add a chat.
    AddChat {
        chat: Chat,
        /// Whether add the chat in the top of the list.
        at_top: bool,
    },
    /// Update a chat in place.
    UpdateChat {
        chat: Chat,
        /// Whether move the chat to the top of the list.
        move_to_top: bool,
    },

    /// Select a chat.
    Select(String),
    /// Select a chat by its position.
    SelectPosition(u32),
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
            set_hscrollbar_policy: gtk::PolicyType::Never,
            set_overlay_scrolling: true,
            set_propagate_natural_width: true,

            #[local_ref]
            list_view -> gtk::ListView {
                set_css_classes: &["navigation-sidebar"],
            }
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let model = Self {
            state: ChatListState::default(),
            chat_jid: None,
            list_view_wrapper: TypedListView::new(),
        };

        let selection_model = &model.list_view_wrapper.selection_model;

        // Enable chat row unselect.
        selection_model.set_can_unselect(true);

        // Disable chat row auto-select.
        selection_model.set_autoselect(false);

        let list_view = &model.list_view_wrapper.view;

        let widgets = view_output!();

        let input_sender = sender.input_sender().clone();
        let suppress_selection_clone = model.state.suppress_selection.clone();
        selection_model.connect_selected_item_notify(move |model| {
            if !suppress_selection_clone.load(Ordering::Acquire) {
                let position = model.selected();
                input_sender.emit(ChatListInput::SelectPosition(position));
            }
        });

        AsyncComponentParts { model, widgets }
    }

    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatListInput::AddChat { chat, at_top } => {
                if self.get_index_by_jid(&chat.jid).is_some() {
                    sender.input(ChatListInput::UpdateChat {
                        chat,
                        move_to_top: at_top,
                    });
                } else {
                    self.state.suppress_selection.store(true, Ordering::Release);

                    let last_message = chat
                        .get_last_message()
                        .await
                        .expect("Failed to get chat last message");
                    let unread_count = chat
                        .get_unread_count()
                        .await
                        .map_or(0, |c| u32::try_from(c).unwrap());
                    let avatar_texture = if let Some(ref path) = chat.avatar_path {
                        load_avatar(path).await
                    } else {
                        None
                    };

                    // Build chat row.
                    let row = ChatRow {
                        chat,
                        last_message,
                        unread_count,
                        avatar_texture,
                    };

                    if at_top {
                        self.list_view_wrapper.insert(0, row);
                    } else {
                        self.list_view_wrapper.append(row);
                    }

                    self.state
                        .suppress_selection
                        .store(false, Ordering::Release);
                }
            }
            ChatListInput::UpdateChat { chat, move_to_top } => {
                self.state.suppress_selection.store(true, Ordering::Release);

                if let Some(index) = self.get_index_by_jid(&chat.jid) {
                    let last_message = chat
                        .get_last_message()
                        .await
                        .expect("Failed to get chat last message");
                    let unread_count = chat
                        .get_unread_count()
                        .await
                        .map_or(0, |c| u32::try_from(c).unwrap());
                    let avatar_texture = if let Some(ref path) = chat.avatar_path {
                        load_avatar(path).await
                    } else {
                        None
                    };

                    // Build updated chat row.
                    let updated_row = ChatRow {
                        chat: chat.clone(),
                        last_message,
                        unread_count,
                        avatar_texture,
                    };

                    // Replace the chat row in place.
                    self.list_view_wrapper.remove(index);

                    let new_index = if move_to_top { 0 } else { index };
                    self.list_view_wrapper.insert(new_index, updated_row);

                    // Re-select the row and scroll to top if the selected chat is there.
                    if self.chat_jid.as_deref() == Some(&chat.jid) {
                        self.list_view_wrapper
                            .selection_model
                            .select_item(new_index, true);

                        if move_to_top {
                            if let Some(adj) = self.list_view_wrapper.view.vadjustment() {
                                // Waits for GTK to finish updating the ListView dimensions, and then
                                // snaps the viewport to the top.
                                glib::idle_add_local_once(move || adj.set_value(adj.lower()));
                            }
                        }
                    }
                }

                self.state
                    .suppress_selection
                    .store(false, Ordering::Release);
            }

            ChatListInput::Select(jid) => {
                // Check if the selected chat isn't already selected.
                if self.chat_jid.as_deref() != Some(&jid) {
                    self.chat_jid = Some(jid.clone());
                    let _ = sender.output(ChatListOutput::ChatSelected(jid));
                }
            }
            ChatListInput::SelectPosition(position) => {
                // Get the chat row.
                if let Some(item) = self.list_view_wrapper.get(position) {
                    let row = item.borrow();

                    let jid = row.chat.jid.clone();
                    sender.input(ChatListInput::Select(jid));
                }
            }
            ChatListInput::ClearSelection => {
                if self.chat_jid.is_some()
                    || self
                        .list_view_wrapper
                        .selection_model
                        .selected_item()
                        .is_some()
                {
                    self.chat_jid = None;
                    self.list_view_wrapper.selection_model.unselect_all();
                }
            }
        }
    }
}

impl ChatList {
    /// Find the index by its chat JID.
    fn get_index_by_jid(&self, jid: &str) -> Option<u32> {
        for (i, row) in self.list_view_wrapper.iter().enumerate() {
            if row.borrow().chat.jid == jid {
                return Some(u32::try_from(i).unwrap());
            }
        }

        None
    }
}

/// A single row in the chat history list.
#[derive(Clone, Debug)]
pub struct ChatRow {
    chat: Chat,
    /// The last sent message in the chat.
    last_message: Option<ChatMessage>,
    /// How many messages are unread.
    unread_count: u32,
    avatar_texture: Option<Texture>,
}

pub struct ChatRowWidgets {
    /// Chat avatar.
    avatar: adw::Avatar,
    /// Chat title.
    title_label: gtk::Label,
    /// Chat last message's content.
    subtitle_label: gtk::Label,
    /// Timestamp label (e.g. "14:30").
    timestamp_label: gtk::Label,
    /// Muted icon.
    muted_icon: gtk::Image,
    /// Pinned icon.
    pinned_icon: gtk::Image,
    /// Unread count badge.
    unread_count_badge: gtk::Label,
}

impl RelmListItem for ChatRow {
    type Root = gtk::Box;
    type Widgets = ChatRowWidgets;

    fn setup(_list_item: &gtk::ListItem) -> (Self::Root, Self::Widgets) {
        let root = gtk::Box::builder()
            .spacing(12)
            .orientation(gtk::Orientation::Horizontal)
            .margin_start(4)
            .margin_end(4)
            .margin_top(8)
            .margin_bottom(8)
            .build();

        // Avatar overlay.
        let avatar_overlay = gtk::Overlay::new();
        let avatar = adw::Avatar::builder().size(36).show_initials(true).build();
        avatar_overlay.set_child(Some(&avatar));
        root.append(&avatar_overlay);

        // TODO: online dot

        // Middle text box (title and subtitle).
        let text_box = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .hexpand(true)
            .spacing(2)
            .orientation(gtk::Orientation::Vertical)
            .build();
        root.append(&text_box);

        let title_label = gtk::Label::builder()
            .lines(1)
            .halign(gtk::Align::Start)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        text_box.append(&title_label);

        let subtitle_label = gtk::Label::builder()
            .lines(1)
            .halign(gtk::Align::Start)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["dimmed"])
            .build();
        text_box.append(&subtitle_label);

        // End box.
        let suffix_box = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .spacing(2)
            .orientation(gtk::Orientation::Vertical)
            .build();
        root.append(&suffix_box);

        let suffix_top_box = gtk::Box::builder()
            .halign(gtk::Align::End)
            .spacing(4)
            .orientation(gtk::Orientation::Horizontal)
            .build();
        suffix_box.append(&suffix_top_box);

        let timestamp_label = gtk::Label::builder()
            .css_classes(["dimmed", "caption", "numeric"])
            .build();
        suffix_top_box.append(&timestamp_label);

        let suffix_bottom_box = gtk::Box::builder()
            .halign(gtk::Align::End)
            .spacing(4)
            .orientation(gtk::Orientation::Horizontal)
            .build();
        suffix_box.append(&suffix_bottom_box);

        let muted_icon = gtk::Image::builder()
            .halign(gtk::Align::End)
            .icon_name("speaker-0-symbolic")
            .pixel_size(12)
            .css_classes(["dimmed"])
            .build();
        suffix_bottom_box.append(&muted_icon);

        let pinned_icon = gtk::Image::builder()
            .halign(gtk::Align::End)
            .icon_name("pin-symbolic")
            .pixel_size(12)
            .css_classes(["dimmed"])
            .build();
        suffix_bottom_box.append(&pinned_icon);

        let unread_count_badge = gtk::Label::builder()
            .justify(gtk::Justification::Center)
            .css_classes(["badge", "numeric"])
            .build();
        suffix_bottom_box.append(&unread_count_badge);

        let widgets = ChatRowWidgets {
            avatar,
            title_label,
            subtitle_label,
            timestamp_label,
            muted_icon,
            pinned_icon,
            unread_count_badge,
        };

        (root, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, root: &mut Self::Root) {
        let name = if self.chat.name.trim().is_empty() {
            format_lid_as_number(&self.chat.jid)
        } else {
            self.chat.name.trim().to_string()
        };
        widgets.title_label.set_label(&name);
        root.set_widget_name(&self.chat.jid);

        widgets.avatar.set_text(Some(&self.chat.name));
        widgets.muted_icon.set_visible(self.chat.muted);
        widgets.pinned_icon.set_visible(self.chat.pinned);

        // Load avatar image if available.
        if let texture @ Some(_) = self.avatar_texture.as_ref() {
            widgets.avatar.set_custom_image(texture);
        } else {
            widgets.avatar.set_custom_image(None::<&Texture>);
        }

        if self.unread_count > 0 {
            widgets
                .unread_count_badge
                .set_label(self.unread_count.to_string().as_str());
            widgets.unread_count_badge.set_visible(true);
        } else {
            widgets.unread_count_badge.set_visible(false);
        }

        root.remove_css_class("dimmed");
        widgets.unread_count_badge.remove_css_class("dimmed");

        if self.chat.muted {
            root.add_css_class("dimmed");
            widgets.unread_count_badge.add_css_class("dimmed");
        }

        if let Some(msg) = &self.last_message {
            // Get last message's content.
            let mut content = msg.content.clone();
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

            if let Some(ref name) = msg.sender_name {
                if self.chat.is_group() && !msg.outgoing {
                    content = format!("{name}: {content}");
                    first_line = format!("{}: {first_line}", get_first_name(name));
                } else if msg.outgoing {
                    content = format!("{}: {content}", i18n!("You"));
                    first_line = format!("{}: {first_line}", i18n!("You"));
                }
            }

            widgets.subtitle_label.set_label(&first_line);
            root.set_tooltip_text(Some(&content));

            // Get last message's timestamp.
            let now = Local::now();
            let timestamp = msg.timestamp.with_timezone(&Local);

            let sent_today = (now - timestamp).num_days() == 0;
            let time = if sent_today {
                timestamp.format("%H:%M").to_string()
            } else {
                timestamp.format("%d/%m").to_string()
            };
            widgets.timestamp_label.set_label(&time);
        } else {
            widgets.subtitle_label.set_label("");
            widgets.timestamp_label.set_label("");
            root.set_tooltip_text(None);
        }
    }
}

async fn load_avatar<P: AsRef<Path>>(path: P) -> Option<Texture> {
    let file = gio::File::for_path(&path);

    match file.load_bytes_future().await {
        Ok((bytes, _)) => Texture::from_bytes(&bytes).ok(),
        Err(e) => {
            tracing::error!(
                "Failed to load avatar from {}: {e}",
                path.as_ref().display()
            );

            None
        }
    }
}
