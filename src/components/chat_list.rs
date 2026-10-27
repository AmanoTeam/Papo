use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    collections::{HashMap, VecDeque},
    iter, mem,
    rc::Rc,
    slice,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk::Texture, gio, glib, pango};
use jiff::{Timestamp, Zoned, tz::TimeZone};
use relm4::{
    prelude::*,
    typed_view::list::{RelmListItem, TypedListView},
};

use crate::{
    db::store::SessionStore,
    i18n, i18n_f,
    session::avatar,
    state::{Chat, ChatMessage, MessageStatus, TypingSender},
    utils::{SenderResolution, cmp_chat_order, format_lid_as_number, get_first_name},
    widgets::TypingDots,
};

#[derive(Debug)]
pub struct ChatList {
    db: SessionStore,
    typing: HashMap<String, Vec<TypingSender>>,
    pending_rows: VecDeque<ChatRowData>,
    populate_cycle: bool,
    pending_avatars: HashMap<String, String>,

    chat_jid: Option<String>,
    own_forms: Vec<String>,
    filter_state: Rc<RefCell<ChatListFilter>>,
    select_guard: Cell<bool>,
    list_view_wrapper: TypedListView<ChatRow, gtk::SingleSelection>,
}

#[derive(Debug, Default)]
pub enum ChatListFilter {
    #[default]
    All,
    Groups,
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
    AddChat {
        chat: Chat,
    },
    UpdateChat {
        chat: Chat,
    },
    RemoveChat {
        jid: String,
    },
    PopulateAll(Vec<Chat>),
    UpdateTyping {
        chat_jid: String,
        senders: Vec<TypingSender>,
    },

    ApplyRows(Vec<ChatRowData>),
    ApplyFilter(ChatListFilter),
    UpdateAvatar {
        jid: String,
        avatar_path: String,
    },

    Select(String),
    SelectPosition(u32),
    ClearSelection,

    OwnJids(Vec<String>),
    PumpPending,
}

#[derive(Debug)]
pub enum ChatListOutput {
    ChatSelected(String),
}

#[relm4::component(async, pub)]
impl SimpleAsyncComponent for ChatList {
    type Init = SessionStore;
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

    #[allow(clippy::unused_async_trait_impl)]
    async fn init(
        db: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let mut model = Self {
            db,
            typing: HashMap::new(),
            pending_rows: VecDeque::new(),
            populate_cycle: false,
            pending_avatars: HashMap::new(),

            chat_jid: None,
            own_forms: Vec::new(),
            filter_state: Rc::new(RefCell::new(ChatListFilter::All)),
            select_guard: Cell::new(false),
            list_view_wrapper: TypedListView::new(),
        };

        let selection_model = &model.list_view_wrapper.selection_model;
        selection_model.set_autoselect(false);
        selection_model.set_can_unselect(true);

        let list_view = &model.list_view_wrapper.view;

        let widgets = view_output!();

        let input_sender = sender.input_sender().clone();
        selection_model.connect_selected_item_notify(move |model| {
            input_sender.emit(ChatListInput::SelectPosition(model.selected()));
        });

        let filter_state = model.filter_state.clone();
        model.list_view_wrapper.add_filter(move |row| {
            let has_message = row.data.last_message.is_some();

            match *filter_state.borrow() {
                ChatListFilter::All => has_message,
                ChatListFilter::Groups => has_message && row.data.chat.is_group(),
                ChatListFilter::Unreads => has_message && row.data.unread_count > 0,
            }
        });

        AsyncComponentParts { model, widgets }
    }

    #[allow(clippy::too_many_lines)]
    async fn update(&mut self, input: Self::Input, sender: AsyncComponentSender<Self>) {
        match input {
            ChatListInput::AddChat { chat } => {
                if self.get_index_by_jid(&chat.jid).is_some() {
                    sender.input(ChatListInput::UpdateChat { chat });
                } else {
                    let mut last_message = match chat.get_last_message(&self.db).await {
                        Ok(message) => message,
                        Err(e) => {
                            tracing::error!("Failed to get chat last message: {}", e);
                            None
                        }
                    };

                    // Previews must match what the chat view would
                    // render on open.
                    if let Some(message) = last_message.as_mut() {
                        let forms = self.own_forms.clone();
                        let mut resolution = SenderResolution::new(&self.db, forms).await;
                        resolution
                            .learn_identity_pairs(&self.db, &chat, slice::from_ref(message))
                            .await;
                        resolution.resolve_message(&chat, message);
                    }

                    let unread_count = chat
                        .get_unread_count(&self.db)
                        .await
                        .map_or(0, |c| u32::try_from(c).unwrap());

                    // Build chat row.
                    let key = (chat.pinned, chat.pinned_at, chat.last_message_time);
                    let row = self.build_row(ChatRowData {
                        chat,
                        last_message,
                        unread_count,
                    });

                    let index = self.sorted_index(key);
                    self.list_view_wrapper.insert(index, row);
                }
            }
            ChatListInput::UpdateChat { chat } => {
                let Some(index) = self.get_index_by_jid(&chat.jid) else {
                    // A populate batch may still be in flight; insert the
                    // chat directly instead of dropping the update.
                    sender.input(ChatListInput::AddChat { chat });
                    return;
                };

                let pinned = chat.pinned;
                let pinned_at = chat.pinned_at;
                let chat_jid = chat.jid.clone();
                let key = (pinned, pinned_at, chat.last_message_time);

                let old_pins = self.list_view_wrapper.iter().nth(index as usize).map_or(
                    (pinned, pinned_at),
                    |item| {
                        let row = item.borrow();
                        (row.data.chat.pinned, row.data.chat.pinned_at)
                    },
                );
                let was_at_top = self
                    .list_view_wrapper
                    .view
                    .vadjustment()
                    .is_some_and(|adj| adj.value() <= adj.lower() + 72.0);

                let last_message = match chat.get_last_message(&self.db).await {
                    Ok(message) => message,
                    Err(e) => {
                        tracing::error!("Failed to get chat last message: {e}");
                        None
                    }
                };
                let unread_count = chat
                    .get_unread_count(&self.db)
                    .await
                    .map_or(0, |c| u32::try_from(c).unwrap());

                // Build updated chat row.
                let updated_row = self.build_row(ChatRowData {
                    chat,
                    last_message,
                    unread_count,
                });

                // A row that does not move is replaced in one splice:
                // remove+insert bounces everything below it twice per
                // update, which is the flash a busy list shows.
                let key_at = |i: u32| {
                    self.list_view_wrapper.iter().nth(i as usize).map(|item| {
                        let row = item.borrow();
                        (
                            row.data.chat.pinned,
                            row.data.chat.pinned_at,
                            row.data.chat.last_message_time,
                        )
                    })
                };
                let keeps_position = (index == 0
                    || key_at(index - 1)
                        .is_none_or(|k| cmp_chat_order(&key, &k) != Ordering::Less))
                    && (key_at(index + 1)
                        .is_none_or(|k| cmp_chat_order(&k, &key) != Ordering::Less));

                let store = self.store();
                let object = glib::BoxedAnyObject::new(updated_row);

                // Selection notifications are suppressed while our own
                // maintenance mutates the store: the model tracks rows
                // by index, so a moving splice would otherwise slide
                // the selection onto a neighbor chat as if clicked.
                self.select_guard.set(true);

                if keeps_position {
                    store.splice(index, 1, &[object]);
                } else {
                    store.splice(index, 1, &[] as &[glib::Object]);
                    let target = self.sorted_index(key);
                    store.splice(target, 0, &[object]);
                }

                // Re-select the row if it is the selected chat.
                if self.chat_jid.as_deref() == Some(&chat_jid)
                    && let Some(position) = self.get_visible_index_by_jid(&chat_jid)
                {
                    self.list_view_wrapper
                        .selection_model
                        .select_item(position, true);
                }

                self.select_guard.set(false);

                // A deliberate pin change deserves to be seen; a reader
                // parked at the top stays anchored there through the
                // splice churn.
                if (old_pins != (pinned, pinned_at) || was_at_top)
                    && let Some(adj) = self.list_view_wrapper.view.vadjustment()
                {
                    glib::idle_add_local_once({
                        let adj = adj.clone();
                        move || adj.set_value(adj.lower())
                    });

                    // The idle shot can land before the list lays out
                    // the spliced row; a second pins the settled bounds.
                    glib::timeout_add_local_once(Duration::from_millis(120), move || {
                        adj.set_value(adj.lower());
                    });
                }
            }
            ChatListInput::RemoveChat { jid } => {
                if let Some(index) = self.get_index_by_jid(&jid) {
                    self.list_view_wrapper.remove(index);
                    if self.chat_jid.as_deref() == Some(&jid) {
                        self.chat_jid = None;
                    }
                }
            }
            ChatListInput::PopulateAll(chats) => {
                // A populate cycle re-opens the list: its rows land in
                // slices, and the frame stays pinned at the top through
                // every one of them (and past later live arrivals).
                self.populate_cycle = true;

                let db = self.db.clone();
                let sender = sender.clone();
                let own_forms = self.own_forms.clone();
                relm4::spawn(async move {
                    const BATCH: usize = 100;
                    let mut batch = Vec::with_capacity(BATCH);

                    // One snapshot for every preview: old rows carry
                    // stale stored names, and the list must match
                    // what the chat view would render on open.
                    let mut resolution = SenderResolution::new(&db, own_forms).await;

                    for chat in chats {
                        let mut last_message = match chat.get_last_message(&db).await {
                            Ok(message) => message,
                            Err(e) => {
                                tracing::error!("Failed to get chat last message: {e}");
                                None
                            }
                        };

                        if let Some(message) = last_message.as_mut() {
                            resolution
                                .learn_identity_pairs(&db, &chat, slice::from_ref(message))
                                .await;
                            resolution.resolve_message(&chat, message);
                        }

                        let unread_count = chat
                            .get_unread_count(&db)
                            .await
                            .map_or(0, |c| u32::try_from(c).unwrap());
                        batch.push(ChatRowData {
                            chat,
                            last_message,
                            unread_count,
                        });

                        if batch.len() >= BATCH {
                            sender.input(ChatListInput::ApplyRows(mem::take(&mut batch)));
                        }
                    }

                    if !batch.is_empty() {
                        sender.input(ChatListInput::ApplyRows(batch));
                    }
                });
            }
            ChatListInput::UpdateTyping { chat_jid, senders } => {
                if senders.is_empty() {
                    self.typing.remove(&chat_jid);
                } else {
                    self.typing.insert(chat_jid.clone(), senders);
                }

                if let Some(index) = self.get_index_by_jid(&chat_jid) {
                    self.rebind_typing_row(&chat_jid, index).await;

                    // Re-select the row.
                    if self.chat_jid.as_deref() == Some(&chat_jid)
                        && let Some(position) = self.get_visible_index_by_jid(&chat_jid)
                    {
                        self.list_view_wrapper
                            .selection_model
                            .select_item(position, true);
                    }
                }
            }

            ChatListInput::ApplyRows(rows) => {
                self.pending_rows.extend(rows);
                sender.input(ChatListInput::PumpPending);
            }
            ChatListInput::ApplyFilter(filter) => {
                // Swap the shared state and invalidate the whole store so
                // the persistent filter re-evaluates every row; no filter
                // model is rebuilt.
                *self.filter_state.borrow_mut() = filter;
                let n_items = self.store().n_items();
                self.store().items_changed(0, n_items, n_items);

                // Re-select the row under the new filter.
                if let Some(jid) = self.chat_jid.as_deref()
                    && let Some(position) = self.get_visible_index_by_jid(jid)
                {
                    self.list_view_wrapper
                        .selection_model
                        .select_item(position, true);
                }

                // Filters take the reader back to the top.
                if let Some(adj) = self.list_view_wrapper.view.vadjustment() {
                    glib::idle_add_local_once(move || adj.set_value(adj.lower()));
                }
            }
            ChatListInput::UpdateAvatar { jid, avatar_path } => {
                let Some(index) = self.get_index_by_jid(&jid) else {
                    // The populate batches may still be in flight; park
                    // the avatar until the row is built.
                    self.pending_avatars.insert(jid, avatar_path);
                    return;
                };

                if let Some(item) = self.list_view_wrapper.iter().nth(index as usize) {
                    let data = {
                        let row = item.borrow();
                        let mut data = row.data.clone();
                        data.chat.avatar_path = Some(avatar_path);
                        data
                    };

                    let updated_row = self.build_row(data);
                    let object = glib::BoxedAnyObject::new(updated_row);
                    self.store().splice(index, 1, &[object]);

                    // Re-select the row.
                    if self.chat_jid.as_deref() == Some(&jid)
                        && let Some(position) = self.get_visible_index_by_jid(&jid)
                    {
                        self.list_view_wrapper
                            .selection_model
                            .select_item(position, true);
                    }
                }
            }

            ChatListInput::Select(jid) => {
                if self.chat_jid.as_deref() != Some(&jid) {
                    self.select_guard.set(true);

                    // The open chat hides its typing line, and the chat
                    // that just stopped being open shows it again.
                    let previous = self.chat_jid.replace(jid.clone());
                    for touched in previous.into_iter().chain(iter::once(jid.clone())) {
                        if let Some(index) = self.get_index_by_jid(&touched) {
                            self.rebind_typing_row(&touched, index).await;

                            if touched == jid
                                && let Some(position) = self.get_visible_index_by_jid(&jid)
                            {
                                self.list_view_wrapper
                                    .selection_model
                                    .select_item(position, true);
                            }
                        }
                    }

                    self.select_guard.set(false);

                    let _ = sender.output(ChatListOutput::ChatSelected(jid));
                }
            }
            ChatListInput::SelectPosition(position) => {
                // Selection changes set off by our own splices land
                // here too; only gestures from real users may re-open.
                if self.select_guard.get() {
                    return;
                }

                if let Some(item) = self.list_view_wrapper.get_visible(position) {
                    let row = item.borrow();

                    let jid = row.data.chat.jid.clone();
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

            ChatListInput::OwnJids(forms) => {
                self.own_forms = forms;
            }
            ChatListInput::PumpPending => {
                // Draining a bounded slice per message keeps every other
                // input ahead of the remainder: the self-queued pump goes
                // to the back of the channel, behind the user's clicks.
                const ROWS_PER_SLICE: usize = 40;

                let mut built = 0;
                while built < ROWS_PER_SLICE
                    && let Some(mut data) = self.pending_rows.pop_front()
                {
                    // A live update may have inserted the chat while the
                    // populate batches were still in flight.
                    if self.get_index_by_jid(&data.chat.jid).is_some() {
                        continue;
                    }

                    // Likewise, an avatar may have arrived while the row
                    // was still being built in the background.
                    if let Some(path) = self.pending_avatars.remove(&data.chat.jid) {
                        data.chat.avatar_path = Some(path);
                    }

                    // Populated rows arrive in sort order, but live
                    // arrivals may interleave: every insert walks to its
                    // own sorted position.
                    let key = (
                        data.chat.pinned,
                        data.chat.pinned_at,
                        data.chat.last_message_time,
                    );
                    let index = self.sorted_index(key);
                    self.list_view_wrapper.insert(index, self.build_row(data));

                    built += 1;
                }

                // Every populate slice re-pins the top: on fast-sync
                // starts, live rows from the stream can land before the
                // first batch, so gating on an empty list strands the
                // frame wherever the storm left it.
                if self.populate_cycle
                    && built > 0
                    && let Some(adj) = self.list_view_wrapper.view.vadjustment()
                {
                    glib::idle_add_local_once({
                        let adj = adj.clone();
                        move || adj.set_value(adj.lower())
                    });

                    // The idle shot can land before the list lays out
                    // the fresh rows; a second pins the settled bounds.
                    glib::timeout_add_local_once(Duration::from_millis(120), move || {
                        adj.set_value(adj.lower());
                    });
                }

                if self.pending_rows.is_empty() {
                    self.populate_cycle = false;
                } else {
                    sender.input(ChatListInput::PumpPending);
                }
            }
        }
    }
}

impl ChatList {
    /// The backing `gio::ListStore`, recovered by descending the
    /// selection and filter model chain, so row updates can be spliced
    /// with a single items-changed emission.
    fn store(&self) -> gio::ListStore {
        let mut model = self.list_view_wrapper.selection_model.model();
        while let Some(current) = model {
            if let Ok(store) = current.clone().downcast::<gio::ListStore>() {
                return store;
            }

            model = current
                .downcast::<gtk::FilterListModel>()
                .ok()
                .and_then(|filter_model| filter_model.model());
        }

        unreachable!("selection model chain wraps the raw store")
    }

    /// The position a top-of-list insertion lands at: pinned chats hold
    /// their prefix, unpinned arrivals slot in below them.
    fn top_index(&self, pinned: bool) -> u32 {
        if pinned {
            return 0;
        }

        for (position, row) in self.list_view_wrapper.iter().enumerate() {
            if !row.borrow().data.chat.pinned {
                return u32::try_from(position).expect("row count fits u32");
            }
        }

        self.store().n_items()
    }

    /// The position a row with this order key belongs at: before the
    /// first existing row it outranks, or the end of the list — the
    /// shared comparator made positional, so any arrival order lands
    /// sorted.
    fn sorted_index(&self, key: (bool, Option<i64>, Timestamp)) -> u32 {
        for (position, row) in self.list_view_wrapper.iter().enumerate() {
            let chat = &row.borrow().data.chat;
            if cmp_chat_order(&key, &(chat.pinned, chat.pinned_at, chat.last_message_time))
                == Ordering::Less
            {
                return u32::try_from(position).expect("row count fits u32");
            }
        }

        self.store().n_items()
    }

    fn get_index_by_jid(&self, jid: &str) -> Option<u32> {
        for (i, row) in self.list_view_wrapper.iter().enumerate() {
            if row.borrow().data.chat.jid == jid {
                return Some(u32::try_from(i).unwrap());
            }
        }

        None
    }

    /// Returns the chat position in the visible (filtered) model, or `None`
    /// when the row is filtered out or absent.
    fn get_visible_index_by_jid(&self, jid: &str) -> Option<u32> {
        let model = &self.list_view_wrapper.selection_model;

        for position in 0..model.n_items() {
            if let Some(item) = self.list_view_wrapper.get_visible(position)
                && item.borrow().data.chat.jid == jid
            {
                return Some(position);
            }
        }

        None
    }

    fn typing_row_data(&self, jid: &str) -> (bool, Vec<TypingSender>) {
        // The open chat already renders its typists above the message
        // composer; its list row stays on the message preview.
        if self.chat_jid.as_deref() == Some(jid) {
            return (false, Vec::new());
        }

        match self.typing.get(jid) {
            Some(senders) if !senders.is_empty() => (true, senders.clone()),
            _ => (false, Vec::new()),
        }
    }

    /// Rebuilds the row at `index` from its stored data and the
    /// current typing state, splicing it back in place.
    async fn rebind_typing_row(&self, jid: &str, index: u32) {
        let Some(item) = self.list_view_wrapper.iter().nth(index as usize) else {
            return;
        };
        let (chat, mut last_message, unread_count, avatar_texture) = {
            let row = item.borrow();
            (
                row.data.chat.clone(),
                row.data.last_message.clone(),
                row.data.unread_count,
                row.avatar_texture.clone(),
            )
        };

        if let Ok(fresh) = chat.get_last_message(&self.db).await {
            last_message = fresh;
        }

        let (is_typing, typing_senders) = self.typing_row_data(jid);
        let updated_row = ChatRow {
            data: ChatRowData {
                chat,
                last_message,
                unread_count,
            },
            is_typing,
            typing_senders,
            avatar_texture,
        };

        let object = glib::BoxedAnyObject::new(updated_row);
        self.store().splice(index, 1, &[object]);
    }

    /// Builds a row from precomputed data; no database access, so it is
    /// safe to call while keeping the input queue short.
    fn build_row(&self, data: ChatRowData) -> ChatRow {
        let avatar_texture = data.chat.avatar_path.as_ref().and_then(avatar::texture);
        let (is_typing, typing_senders) = self.typing_row_data(&data.chat.jid);

        ChatRow {
            data,
            is_typing,
            typing_senders,
            avatar_texture,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatRow {
    data: ChatRowData,
    is_typing: bool,
    typing_senders: Vec<TypingSender>,
    avatar_texture: Option<Texture>,
}

impl ChatRow {
    fn bind_avatar(&self, widgets: &ChatRowWidgets, root: &gtk::Box) {
        let texture = self.avatar_texture.clone().or_else(|| {
            self.data
                .chat
                .avatar_path
                .as_ref()
                .and_then(avatar::texture)
        });
        if let texture @ Some(_) = texture.as_ref() {
            widgets.avatar.set_custom_image(texture);
        } else {
            widgets.avatar.set_custom_image(None::<&Texture>);

            if let Some(path) = self.data.chat.avatar_path.clone() {
                let jid = self.data.chat.jid.clone();
                let root = root.clone();
                let avatar = widgets.avatar.clone();

                glib::spawn_future_local(async move {
                    if let Some(texture) = avatar::load(&path).await
                        && root.widget_name() == jid
                    {
                        avatar.set_custom_image(Some(&texture));
                    }
                });
            }
        }
    }

    fn typing_text(&self) -> String {
        let recording = self.typing_senders.iter().any(|s| s.recording);
        if self.typing_senders.len() == 1 && recording {
            i18n_f!(
                "{0} is recording audio...",
                self.typing_senders[0].name.clone()
            )
        } else if self.typing_senders.len() == 1 {
            i18n_f!("{0} is typing...", self.typing_senders[0].name.clone())
        } else if recording {
            i18n_f!(
                "{0} people are recording audio...",
                self.typing_senders.len()
            )
        } else {
            i18n_f!("{0} people are typing...", self.typing_senders.len())
        }
    }
}

#[derive(Clone, Debug)]
pub struct ChatRowData {
    chat: Chat,
    last_message: Option<ChatMessage>,
    unread_count: u32,
}

pub struct ChatRowWidgets {
    avatar: adw::Avatar,
    muted_icon: gtk::Image,
    pinned_icon: gtk::Image,
    status_icon: gtk::Image,
    typing_box: gtk::Box,
    suffix_dots: TypingDots,
    title_label: gtk::Label,
    typing_label: gtk::Label,
    subtitle_label: gtk::Label,
    timestamp_label: gtk::Label,
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

        let (typing_box, typing_label) = build_typing_widgets();
        text_box.append(&typing_box);

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

        let muted_icon = build_suffix_icon("speaker-cross-symbolic");
        suffix_bottom_box.append(&muted_icon);

        let pinned_icon = build_suffix_icon("view-pin-symbolic");
        suffix_bottom_box.append(&pinned_icon);

        let unread_count_badge = gtk::Label::builder()
            .justify(gtk::Justification::Center)
            .css_classes(["badge", "numeric"])
            .build();
        suffix_bottom_box.append(&unread_count_badge);

        let suffix_dots = TypingDots::new();
        suffix_dots.set_valign(gtk::Align::Center);
        suffix_dots.add_css_class("dimmed");
        suffix_dots.set_visible(false);
        suffix_bottom_box.append(&suffix_dots);

        let widgets = ChatRowWidgets {
            avatar,
            muted_icon,
            pinned_icon,
            status_icon,
            typing_box,
            suffix_dots,
            title_label,
            typing_label,
            subtitle_label,
            timestamp_label,
            unread_count_badge,
        };

        (root, widgets)
    }

    fn bind(&mut self, widgets: &mut Self::Widgets, root: &mut Self::Root) {
        let name = if self.data.chat.name.trim().is_empty() {
            format_lid_as_number(&self.data.chat.jid)
        } else {
            self.data.chat.name.trim().to_string()
        };
        widgets.title_label.set_label(&name);
        root.set_widget_name(&self.data.chat.jid);

        widgets.avatar.set_text(Some(&self.data.chat.name));
        widgets.muted_icon.set_visible(self.data.chat.muted);
        widgets.pinned_icon.set_visible(self.data.chat.pinned);

        // Avatar textures decode lazily: only visible rows pay.
        self.bind_avatar(widgets, root);

        if self.data.unread_count > 0 {
            widgets
                .unread_count_badge
                .set_label(self.data.unread_count.to_string().as_str());
            widgets.unread_count_badge.set_visible(true);
        } else {
            widgets.unread_count_badge.set_visible(false);
        }

        root.remove_css_class("dimmed");
        widgets.unread_count_badge.remove_css_class("dimmed");

        if self.data.chat.muted {
            root.add_css_class("dimmed");
            widgets.unread_count_badge.add_css_class("dimmed");
        }

        widgets.status_icon.set_has_tooltip(false);
        widgets.status_icon.remove_css_class("white");
        widgets.status_icon.remove_css_class("warning");

        let is_group = self.data.chat.is_group();
        let show_typing_row = self.is_typing && is_group;
        widgets.typing_box.set_visible(show_typing_row);
        widgets.subtitle_label.set_visible(!show_typing_row);
        widgets.suffix_dots.set_visible(self.is_typing && !is_group);

        if show_typing_row {
            widgets.typing_label.set_label(&self.typing_text());
        }

        if let Some(msg) = &self.data.last_message {
            let mut content = last_message_content(msg);
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
                && self.data.chat.is_group()
            {
                if msg.outgoing {
                    content = format!("{}: {content}", i18n!("You"));
                    first_line = format!("{}: {first_line}", i18n!("You"));
                } else {
                    let mark = if msg.sender_push { "~" } else { "" };
                    content = format!("{mark}{name}: {content}");
                    first_line = format!("{mark}{}: {first_line}", get_first_name(name));
                }
            }

            widgets.subtitle_label.set_label(&first_line);
            root.set_tooltip_text(Some(&content));

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

            let now = Zoned::now();
            let timestamp = msg.timestamp.to_zoned(TimeZone::system());
            let sent_today = now.date() == timestamp.date();
            let time = if sent_today {
                timestamp.strftime("%H:%M").to_string()
            } else {
                timestamp.strftime("%d/%m").to_string()
            };
            widgets.timestamp_label.set_label(&time);
        } else {
            widgets.subtitle_label.set_label("");
            widgets.timestamp_label.set_label("");
            root.set_tooltip_text(None);
        }
    }
}

fn last_message_content(msg: &ChatMessage) -> String {
    msg.media.as_ref().map_or_else(
        || msg.content.clone(),
        |media| {
            let label = media.r#type.display_label();
            if msg.content.is_empty() {
                label
            } else {
                format!("{label}\n{}", msg.content)
            }
        },
    )
}

fn build_typing_widgets() -> (gtk::Box, gtk::Label) {
    let typing_box = gtk::Box::builder()
        .spacing(4)
        .valign(gtk::Align::Center)
        .build();
    let typing_dots = TypingDots::new();
    typing_dots.add_css_class("dimmed");
    typing_box.append(&typing_dots);
    let typing_label = gtk::Label::builder()
        .lines(1)
        .halign(gtk::Align::Fill)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["dimmed"])
        .width_chars(1)
        .build();
    typing_box.append(&typing_label);
    typing_box.set_visible(false);

    (typing_box, typing_label)
}

fn build_suffix_icon(icon_name: &str) -> gtk::Image {
    gtk::Image::builder()
        .halign(gtk::Align::End)
        .icon_name(icon_name)
        .pixel_size(12)
        .css_classes(["dimmed"])
        .build()
}
