use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use adw::prelude::*;
use chrono::Local;
use gtk::{gdk::Texture, gio};
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
    /// Metadata tracking for each row, mirrors `list_view_wrapper` order.
    /// Used to update pagination cursors when trimming rows.
    row_metadata: VecDeque<ChatRow>,
    /// `ListView` widget wrapper containing all chat rows.
    list_view_wrapper: TypedListView<ChatRow, gtk::SingleSelection>,
}

#[derive(Debug)]
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
        let list_view_wrapper = TypedListView::new();

        let model = Self {
            state: ChatListState {
                searching_chats: Arc::new(AtomicBool::new(false)),
                suppress_selection: Arc::new(AtomicBool::new(false)),
            },
            chat_jid: None,
            row_metadata: VecDeque::new(),
            list_view_wrapper,
        };

        // Enable chat row unselect.
        model
            .list_view_wrapper
            .selection_model
            .set_can_unselect(true);

        // Disable chat row auto-select.
        model
            .list_view_wrapper
            .selection_model
            .set_autoselect(false);

        let list_view = &model.list_view_wrapper.view;

        let widgets = view_output!();

        let input_sender = sender.input_sender().clone();
        let suppress_selection_clone = model.state.suppress_selection.clone();
        model
            .list_view_wrapper
            .selection_model
            .connect_selected_item_notify(move |model| {
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
                if self
                    .list_view_wrapper
                    .find(|row| row.chat.jid == chat.jid)
                    .is_some()
                {
                    sender.input(ChatListInput::UpdateChat {
                        chat,
                        move_to_top: at_top,
                    });
                } else {
                    self.state.suppress_selection.store(true, Ordering::Release);

                    let last_message = RefCell::new(
                        chat.get_last_message()
                            .await
                            .expect("Failed to get chat last message"),
                    );
                    let row = ChatRow {
                        chat,
                        last_message,
                        has_unread_messages: false,
                    };

                    if at_top {
                        self.row_metadata.push_front(row.clone());
                        self.list_view_wrapper.insert(0, row);
                    } else {
                        self.row_metadata.push_back(row.clone());
                        self.list_view_wrapper.append(row);
                    }

                    self.state
                        .suppress_selection
                        .store(false, Ordering::Release);
                }
            }
            ChatListInput::UpdateChat { chat, move_to_top } => {
                self.state.suppress_selection.store(true, Ordering::Release);

                if let Some(index) = self
                    .row_metadata
                    .iter()
                    .position(|row| row.chat.jid == chat.jid)
                    && let Some(row) = self.row_metadata.get_mut(index)
                {
                    // Update chat row.
                    row.chat = chat.clone();

                    if let Ok(last_message) = chat.get_last_message().await {
                        row.last_message = RefCell::new(last_message);
                    }

                    // Replace the chat row in place.
                    if let Ok(index) = u32::try_from(index) {
                        self.list_view_wrapper.remove(index);
                        if move_to_top {
                            self.list_view_wrapper.insert(0, row.clone());
                        } else {
                            self.list_view_wrapper.insert(index, row.clone());
                        }

                        // Check if the updated chat is the selected chat.
                        if self.chat_jid.as_deref() == Some(&row.chat.jid) {
                            self.list_view_wrapper
                                .selection_model
                                .select_item(index, true);
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
                if let Some(row) = self.row_metadata.get(position as usize) {
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

impl ChatList {}

/// A single row in the chat history list.
#[derive(Clone, Debug)]
pub struct ChatRow {
    chat: Chat,
    /// The last sent message in the chat.
    last_message: RefCell<Option<ChatMessage>>,
    /// Whether the chat has unread messages.
    has_unread_messages: bool,
}

pub struct ChatRowWidgets {
    /// Chat avatar.
    avatar: adw::Avatar,
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
    type Root = adw::ActionRow;
    type Widgets = ChatRowWidgets;

    fn setup(_list_item: &gtk::ListItem) -> (Self::Root, Self::Widgets) {
        let row = adw::ActionRow::builder()
            .title_lines(1)
            .subtitle_lines(1)
            .use_markup(false)
            .activatable(true);

        // Avatar overlay.
        let avatar_overlay = gtk::Overlay::new();

        let avatar = adw::Avatar::builder().size(36).show_initials(true).build();
        avatar_overlay.set_child(Some(&avatar));

        // TODO: online dot

        let suffix_box = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .spacing(2)
            .orientation(gtk::Orientation::Vertical)
            .build();

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

        // Root container.
        let root = row.build();
        root.add_prefix(&avatar_overlay);
        root.add_suffix(&suffix_box);

        let widgets = ChatRowWidgets {
            avatar,
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
        root.set_title(&name);
        root.set_widget_name(&self.chat.jid);

        widgets.avatar.set_text(Some(&self.chat.name));
        // Load avatar image if available.
        if let Some(ref path) = self.chat.avatar_path {
            let file = gio::File::for_path(path);

            if let Ok(texture) = Texture::from_file(&file) {
                widgets.avatar.set_custom_image(Some(&texture));
            } else {
                tracing::warn!("Failed to load avatar image from {path}");
            }
        }

        widgets.muted_icon.set_visible(self.chat.muted);
        widgets.pinned_icon.set_visible(self.chat.pinned);
        widgets.unread_count_badge.set_visible(false);

        root.remove_css_class("dimmed");
        widgets.unread_count_badge.remove_css_class("dimmed");

        if self.chat.muted {
            root.add_css_class("dimmed");
            widgets.unread_count_badge.add_css_class("dimmed");
        }

        let root_clone = root.clone();
        let chat_clone = self.chat.clone();
        let last_message_clone = self.last_message.clone();
        let timestamp_label_clone = widgets.timestamp_label.clone();
        let unread_count_badge_clone = widgets.unread_count_badge.clone();
        relm4::spawn_local(async move {
            // Chat's unread count.
            if let Ok(count) = chat_clone.get_unread_count().await
                && count > 0
            {
                unread_count_badge_clone.set_label(count.to_string().as_str());
                unread_count_badge_clone.set_visible(true);
            }

            let last_message = if last_message_clone.borrow().is_some() {
                last_message_clone.take()
            } else {
                chat_clone
                    .get_last_message()
                    .await
                    .expect("Failed to get chat last message")
            };
            if let Some(msg) = last_message {
                // last message's content.
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
                    if chat_clone.is_group() && !msg.outgoing {
                        content = format!("{name}: {content}");
                        first_line = format!("{}: {first_line}", get_first_name(name));
                    } else if msg.outgoing {
                        content = format!("{}: {content}", i18n!("You"));
                        first_line = format!("{}: {first_line}", i18n!("You"));
                    }
                }

                root_clone.set_subtitle(&first_line);
                root_clone.set_tooltip_text(Some(&content));

                // Last message's timestamp.
                let now = Local::now();
                let timestamp = msg.timestamp.with_timezone(&Local);
                let diff = now - timestamp;

                let sent_today = diff.num_days() == 0;
                let time = if sent_today {
                    timestamp.format("%H:%M").to_string()
                } else {
                    timestamp.format("%d/%m").to_string()
                };
                timestamp_label_clone.set_label(&time);

                // Replace row's last message.
                last_message_clone.replace(Some(msg));
            }
        });
    }
}
