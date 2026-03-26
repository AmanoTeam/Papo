use std::path::Path;

use adw::prelude::*;
use chrono::Local;
use gtk::{gdk::Texture, gio, glib, pango};
use relm4::{
    prelude::*,
    typed_view::list::{RelmListItem, TypedListView},
};

use crate::{
    i18n,
    state::{Chat, ChatMessage, MessageStatus},
    utils::{format_lid_as_number, get_first_name},
};

#[derive(Debug)]
pub struct ChatList {
    /// Currently selected chat JID.
    chat_jid: Option<String>,
    /// `ListView` widget wrapper containing all chat rows.
    list_view_wrapper: TypedListView<ChatRow, gtk::SingleSelection>,
}

#[derive(Debug, Default)]
pub enum ChatListFilter {
    /// All existing chat.
    #[default]
    All,
    /// Only groups.
    Groups,
    /// Chats that have unread messages.
    Unreads,
}

impl From<&str> for ChatListFilter {
    fn from(value: &str) -> Self {
        match value {
            "groups" => Self::Groups,
            "unreads" => Self::Unreads,
            _ => Self::default(),
        }
    }
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

    /// Apply a filter.
    ApplyFilter(ChatListFilter),

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
        gtk::Box {
            set_spacing: 2,
            set_orientation: gtk::Orientation::Vertical,

            gtk::ScrolledWindow {
                set_margin_start: 8,
                set_margin_end: 8,
                set_margin_top: 4,
                set_css_classes: &["undershoot-start", "undershoot-end"],
                set_hscrollbar_policy: gtk::PolicyType::External,
                set_vscrollbar_policy: gtk::PolicyType::Never,
                set_propagate_natural_height: true,

                adw::ToggleGroup {
                    set_can_shrink: false,
                    set_css_classes: &["round"],

                    add = adw::Toggle {
                        set_name: Some("all"),
                        set_label: Some(&i18n!("All")),
                    },

                    add = adw::Toggle {
                        set_name: Some("unreads"),
                        set_label: Some(&i18n!("Unreads")),
                    },

                    add = adw::Toggle {
                        set_name: Some("groups"),
                        set_label: Some(&i18n!("Groups")),
                    },

                    set_active_name: Some("all"),
                    connect_active_name_notify[sender] => move |group| {
                        let filter = group.active_name().map_or(ChatListFilter::default(), |tag| tag.as_str().into());
                        sender.input(ChatListInput::ApplyFilter(filter));
                    }
                },

                add_controller = gtk::EventControllerScroll {
                    set_flags: gtk::EventControllerScrollFlags::VERTICAL,
                    set_propagation_phase: gtk::PropagationPhase::Capture,

                    connect_scroll => move |ctrl, _, dy| {
                        if let Some(sw) = ctrl.widget().and_downcast::<gtk::ScrolledWindow>() {
                            let adj = sw.hadjustment();
                            let mut new_value = adj.value() + (dy * 25.0);
                            new_value = new_value.clamp(adj.lower(), adj.upper() - adj.page_size());
                            adj.set_value(new_value);

                            return glib::Propagation::Stop;
                        }

                        glib::Propagation::Proceed
                    }
                }
            },

            gtk::ScrolledWindow {
                set_vexpand: true,
                set_hscrollbar_policy: gtk::PolicyType::Never,
                set_overlay_scrolling: true,

                #[local_ref]
                list_view -> gtk::ListView {
                    set_css_classes: &["navigation-sidebar"],
                },
            }
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let model = Self {
            chat_jid: None,
            list_view_wrapper: TypedListView::new(),
        };

        let selection_model = &model.list_view_wrapper.selection_model;

        // Disabe chat row autoselecet and enable unselect.
        selection_model.set_autoselect(false);
        selection_model.set_can_unselect(true);

        let list_view = &model.list_view_wrapper.view;

        let widgets = view_output!();

        let input_sender = sender.input_sender().clone();
        selection_model.connect_selected_item_notify(move |model| {
            let position = model.selected();
            input_sender.emit(ChatListInput::SelectPosition(position));
        });

        AsyncComponentParts { model, widgets }
    }

    #[allow(clippy::too_many_lines)]
    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatListInput::AddChat { chat, at_top } => {
                if self.get_index_by_jid(&chat.jid).is_some() {
                    sender.input(ChatListInput::UpdateChat {
                        chat,
                        move_to_top: at_top,
                    });
                } else {
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
                }
            }
            ChatListInput::UpdateChat { chat, move_to_top } => {
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

                    let adj = self.list_view_wrapper.view.vadjustment();
                    let saved_scroll = adj.as_ref().map(AdjustmentExt::value);

                    if move_to_top {
                        // Insert the new updated row.
                        self.list_view_wrapper.insert(0, updated_row);
                        let old_index = index + 1;

                        // Re-select the row and scroll to the top if it's the selected chat.
                        if self.chat_jid.as_deref() == Some(&chat.jid) {
                            self.list_view_wrapper.selection_model.select_item(0, true);

                            if let Some(adj) = adj {
                                glib::idle_add_local_once(move || adj.set_value(adj.lower()));
                            }
                        }

                        // Remove the old row.
                        self.list_view_wrapper.remove(old_index);
                    } else {
                        // Update the row in-place.
                        self.list_view_wrapper.remove(index);
                        self.list_view_wrapper.insert(index, updated_row);

                        // Re-select the row.
                        if self.chat_jid.as_deref() == Some(&chat.jid) {
                            self.list_view_wrapper
                                .selection_model
                                .select_item(index, true);
                        }

                        // Scroll back to where it was before.
                        if let (Some(adj), Some(value)) = (adj, saved_scroll) {
                            glib::idle_add_local_once(move || adj.set_value(value));
                        }
                    }
                }
            }

            ChatListInput::ApplyFilter(filter) => {
                // Remove any existing filter to avoid stacking one filter on top of other.
                self.list_view_wrapper.clear_filters();

                match filter {
                    ChatListFilter::All => {
                        // Re-select the row.
                        if let Some(jid) = self.chat_jid.as_deref()
                            && let Some(position) =
                                self.list_view_wrapper.find(|row| row.chat.jid == jid)
                        {
                            self.list_view_wrapper
                                .selection_model
                                .select_item(position, true);
                        }
                    }
                    ChatListFilter::Groups => {
                        self.list_view_wrapper.add_filter(|row| row.chat.is_group());
                    }
                    ChatListFilter::Unreads => self
                        .list_view_wrapper
                        .add_filter(|row| row.unread_count > 0),
                }
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
                if let Some(item) = self.list_view_wrapper.get_visible(position) {
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
    /// Message status icon (e.g. "Sending", "Sent").
    status_icon: gtk::Image,
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
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .spacing(2)
            .orientation(gtk::Orientation::Vertical)
            .width_request(0)
            .build();
        root.append(&text_box);

        let title_label = gtk::Label::builder()
            .lines(1)
            .halign(gtk::Align::Fill)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(1)
            .build();
        text_box.append(&title_label);

        let subtitle_label = gtk::Label::builder()
            .lines(1)
            .halign(gtk::Align::Fill)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["dimmed"])
            .width_chars(1)
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

        let status_icon = gtk::Image::builder()
            .pixel_size(12)
            .css_classes(["dimmed", "status-icon"])
            .build();
        suffix_top_box.append(&status_icon);

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
            status_icon,
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

        widgets.status_icon.set_has_tooltip(false);
        widgets.status_icon.remove_css_class("white");
        widgets.status_icon.remove_css_class("warning");

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

            if let Some(ref name) = msg.sender_name
                && self.chat.is_group()
            {
                if msg.outgoing {
                    content = format!("{}: {content}", i18n!("You"));
                    first_line = format!("{}: {first_line}", i18n!("You"));
                } else {
                    content = format!("{name}: {content}");
                    first_line = format!("{}: {first_line}", get_first_name(name));
                }
            }

            widgets.subtitle_label.set_label(&first_line);
            root.set_tooltip_text(Some(&content));

            // Get last message's status.
            if msg.outgoing {
                widgets.status_icon.set_visible(true);
                widgets
                    .status_icon
                    .set_icon_name(Some(msg.status.icon_name()));
                match msg.status {
                    MessageStatus::Read => {
                        widgets.status_icon.add_css_class("white");
                    }
                    MessageStatus::Failed => {
                        widgets.status_icon.add_css_class("warning");
                    }
                    _ => {}
                }
            } else {
                widgets.status_icon.set_visible(false);
            }

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
