use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fmt,
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gio, glib, subclass::prelude::*};
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use relm4::{prelude::*, typed_view::list::RelmListItem};

use super::rows::ChatRow;
use crate::{
    components::ChatViewInput,
    state::{ChatMessage, MessageStatus},
    utils::{bare_jid, format_date_label},
};

/// The canonical identity key for author grouping: a LID-keyed
/// sender maps onto its phone-number identity when a learned
/// mapping pair covers it, so one person's rows form one group
/// regardless of which wire form each row was keyed by.
fn identity_key(jid: &str, identities: &HashMap<String, String>) -> String {
    let bare = bare_jid(jid);

    match bare.strip_suffix("@lid") {
        Some(digits) => identities
            .get(digits)
            .map_or_else(|| bare, |phone| format!("{phone}@s.whatsapp.net")),
        None => bare,
    }
}

/// Whether two messages render in the same author group.
fn same_group(
    a: &ChatMessage,
    b: &ChatMessage,
    identities: &HashMap<String, String>,
    is_group_chat: bool,
) -> bool {
    if !is_group_chat {
        // A private chat has exactly two authors and the wire may spell
        // the same peer under several identities.
        return a.outgoing == b.outgoing;
    }

    (a.outgoing && b.outgoing)
        || (!a.outgoing
            && !b.outgoing
            && identity_key(&a.sender_jid, identities) == identity_key(&b.sender_jid, identities))
}

/// The local calendar day of a message, used for section boundaries.
fn message_day(message: &ChatMessage) -> Date {
    message.timestamp.to_zoned(TimeZone::system()).date()
}

mod imp {
    use super::{Cast, Date, RefCell, StaticType, gio, glib, gtk};
    use gtk::subclass::prelude::*;

    /// One row: the boxed `ChatRow` plus its precomputed calendar day,
    /// so section boundaries stay a cheap ordered scan.
    pub struct TimelineItem {
        pub object: glib::BoxedAnyObject,
        pub day: Date,
    }

    #[derive(Default)]
    pub struct TimelineModel {
        pub rows: RefCell<Vec<TimelineItem>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TimelineModel {
        const NAME: &'static str = "PapoTimelineModel";
        type Type = super::TimelineModel;
        type Interfaces = (gio::ListModel, gtk::SectionModel);
    }

    impl ObjectImpl for TimelineModel {}

    impl ListModelImpl for TimelineModel {
        fn item_type(&self) -> glib::Type {
            glib::BoxedAnyObject::static_type()
        }

        fn n_items(&self) -> u32 {
            u32::try_from(self.rows.borrow().len()).expect("row count fits u32")
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let rows = self.rows.borrow();
            rows.get(usize::try_from(position).ok()?)
                .map(|item| item.object.clone().upcast())
        }
    }

    impl SectionModelImpl for TimelineModel {
        fn section(&self, position: u32) -> (u32, u32) {
            let rows = self.rows.borrow();
            let len = u32::try_from(rows.len()).expect("row count fits u32");
            if position >= len {
                return (len, u32::MAX);
            }

            let day = rows[usize::try_from(position).expect("checked above")].day;
            let first_in = rows.partition_point(|row| row.day < day);
            let first_out = rows.partition_point(|row| row.day <= day);
            (
                u32::try_from(first_in).expect("row count fits u32"),
                u32::try_from(first_out).expect("row count fits u32"),
            )
        }
    }
}

glib::wrapper! {
    /// The chat timeline as a flat list of rows, with calendar days
    /// exposed as [`gtk::SectionModel`] sections: day headers are
    /// rendered by the header factory instead of being list items, so
    /// a prepend is a pure insertion of message rows.
    pub struct TimelineModel(ObjectSubclass<imp::TimelineModel>)
        @implements gio::ListModel, gtk::SectionModel;
}

/// Key under a row root keeps its widgets between factory callbacks.
const ROW_WIDGETS_KEY: &str = "papo-row-widgets";

impl TimelineModel {
    fn new() -> Self {
        glib::Object::new()
    }

    fn len(&self) -> u32 {
        u32::try_from(self.imp().rows.borrow().len()).expect("row count fits u32")
    }

    fn row(&self, index: u32) -> Option<ChatRow> {
        let rows = self.imp().rows.borrow();
        rows.get(usize::try_from(index).ok()?)
            .map(|item| item.object.borrow::<ChatRow>().clone())
    }

    fn last_row(&self) -> Option<ChatRow> {
        let rows = self.imp().rows.borrow();
        rows.last()
            .map(|item| item.object.borrow::<ChatRow>().clone())
    }

    /// Index of the first row matching `predicate`.
    fn position_row(&self, predicate: impl Fn(&ChatRow) -> bool) -> Option<u32> {
        let rows = self.imp().rows.borrow();
        rows.iter()
            .position(|item| predicate(&item.object.borrow::<ChatRow>()))
            .and_then(|index| u32::try_from(index).ok())
    }

    fn any_row(&self, predicate: impl Fn(&ChatRow) -> bool) -> bool {
        let rows = self.imp().rows.borrow();
        rows.iter()
            .any(|item| predicate(&item.object.borrow::<ChatRow>()))
    }

    /// The precomputed calendar day of the row at `index`.
    fn row_day(&self, index: u32) -> Option<Date> {
        let rows = self.imp().rows.borrow();
        rows.get(usize::try_from(index).ok()?).map(|item| item.day)
    }

    // Mutations. Each one releases the borrow before emitting
    // items-changed, so GTK never observes the model mid-update.

    /// Appends a batch of rows to the bottom, in chronological order.
    fn append_batch(&self, batch: Vec<(ChatRow, Date)>) {
        if batch.is_empty() {
            return;
        }

        // Scrub any section tiles pending from earlier mutations BEFORE
        // this one: the rescan's conversion walk settles them safely,
        // while the walk inside the items_changed below steps across
        // pendings and dies. The span covers the pre-mutation tree,
        // matching what the announcement-driven tiles believe.
        let stale_len = u32::try_from(self.imp().rows.borrow().len()).expect("row count fits u32");
        if stale_len > 0 {
            self.sections_changed(0, stale_len);
        }

        let (position, added) = {
            let mut rows = self.imp().rows.borrow_mut();
            let position = rows.len();
            let added = batch.len();
            rows.extend(batch.into_iter().map(|(row, day)| imp::TimelineItem {
                object: glib::BoxedAnyObject::new(row),
                day,
            }));
            (position, added)
        };

        let position = u32::try_from(position).expect("row count fits u32");
        let added = u32::try_from(added).expect("row count fits u32");
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, 0, added);
        self.sections_changed(0, position + added);
    }

    /// Prepends a batch of rows to the top, in chronological order
    /// (oldest first); a single pure insertion of the whole batch.
    fn prepend_batch(&self, batch: Vec<(ChatRow, Date)>) {
        if batch.is_empty() {
            return;
        }

        // Same pre-mutation scrub as append_batch: settle pending
        // section tiles before the walk inside items_changed can
        // cross them.
        let stale_len = u32::try_from(self.imp().rows.borrow().len()).expect("row count fits u32");
        if stale_len > 0 {
            self.sections_changed(0, stale_len);
        }

        let added = {
            let mut rows = self.imp().rows.borrow_mut();
            let added = batch.len();
            rows.splice(
                0..0,
                batch.into_iter().map(|(row, day)| imp::TimelineItem {
                    object: glib::BoxedAnyObject::new(row),
                    day,
                }),
            );
            added
        };

        let added = u32::try_from(added).expect("row count fits u32");
        self.upcast_ref::<gio::ListModel>()
            .items_changed(0, 0, added);

        let n_items = u32::try_from(self.imp().rows.borrow().len()).expect("row count fits u32");
        self.sections_changed(0, n_items);
    }

    /// Replaces the row at `index`, keeping its position.
    fn replace(&self, index: u32, row: ChatRow, day: Date) {
        let Some(slot) = usize::try_from(index).ok() else {
            return;
        };

        let replaced = {
            let mut rows = self.imp().rows.borrow_mut();
            let Some(current) = rows.get_mut(slot) else {
                return;
            };
            *current = imp::TimelineItem {
                object: glib::BoxedAnyObject::new(row),
                day,
            };
            true
        };

        if replaced {
            self.upcast_ref::<gio::ListModel>()
                .items_changed(index, 1, 1);
        }
    }

    /// Removes the row at `index` in one splice, settling pending
    /// section tiles around the shift first.
    fn remove(&self, index: u32) {
        let stale_len = u32::try_from(self.imp().rows.borrow().len()).expect("row count fits u32");
        if stale_len == 0 {
            return;
        }

        self.sections_changed(0, stale_len);

        let removed = {
            let mut rows = self.imp().rows.borrow_mut();
            usize::try_from(index)
                .ok()
                .filter(|slot| *slot < rows.len())
                .map(|slot| rows.remove(slot))
                .is_some()
        };

        if removed {
            self.upcast_ref::<gio::ListModel>()
                .items_changed(index, 1, 0);
            self.sections_changed(0, stale_len - 1);
        }
    }
}

/// Builds the message row factory, reusing the row widgets and binding
/// code behind [`RelmListItem`]. Mirrors the widget-stash pattern of
/// `relm4`'s own typed list factory. The input channel lets row binds
/// ask the component for lazy work such as sender avatar fetches.
fn build_row_factory(
    input: Option<relm4::Sender<ChatViewInput>>,
    anchors: Rc<RefCell<HashMap<String, glib::WeakRef<gtk::Widget>>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(move |_, list_item| {
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("Needs to be ListItem");

        let (root, mut widgets) = <ChatRow as RelmListItem>::setup(list_item);
        if let Some(input) = &input {
            widgets.set_input(input);
        }

        // SAFETY: the widgets live as long as `root`, which the list
        // item owns; they are only touched through steals that hand
        // the value back before this object can be destroyed.
        unsafe { root.set_data(ROW_WIDGETS_KEY, widgets) };
        list_item.set_child(Some(&root));
    });

    factory.connect_bind(move |_, list_item| {
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("Needs to be ListItem");

        let Some(item) = list_item.item() else {
            return;
        };
        let Some(wrapper) = item.downcast_ref::<glib::BoxedAnyObject>() else {
            return;
        };
        let Some(mut root) = list_item
            .child()
            .and_downcast::<<ChatRow as RelmListItem>::Root>()
        else {
            return;
        };

        let mut row = wrapper.borrow_mut::<ChatRow>();
        // SAFETY: as in setup — the stash lives as long as the root.
        let mut widgets =
            unsafe { root.steal_data::<<ChatRow as RelmListItem>::Widgets>(ROW_WIDGETS_KEY) }
                .expect("row widgets stashed in setup");
        row.bind(&mut widgets, &mut root);

        if let ChatRow::Message { message, .. } = &*row {
            let anchor = glib::WeakRef::new();
            anchor.set(Some(root.upcast_ref::<gtk::Widget>()));
            anchors
                .borrow_mut()
                .insert(message.server_id.clone(), anchor);
        }

        unsafe { root.set_data(ROW_WIDGETS_KEY, widgets) };
    });

    factory.connect_unbind(|_, list_item| {
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("Needs to be ListItem");

        let Some(item) = list_item.item() else {
            return;
        };
        let Some(wrapper) = item.downcast_ref::<glib::BoxedAnyObject>() else {
            return;
        };
        let Some(mut root) = list_item
            .child()
            .and_downcast::<<ChatRow as RelmListItem>::Root>()
        else {
            return;
        };

        let mut row = wrapper.borrow_mut::<ChatRow>();
        // SAFETY: as in setup — the stash lives as long as the root.
        let mut widgets =
            unsafe { root.steal_data::<<ChatRow as RelmListItem>::Widgets>(ROW_WIDGETS_KEY) }
                .expect("row widgets stashed in setup");
        row.unbind(&mut widgets, &mut root);
        unsafe { root.set_data(ROW_WIDGETS_KEY, widgets) };
    });

    factory.connect_teardown(|_, list_item| {
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("Needs to be ListItem");
        <ChatRow as RelmListItem>::teardown(list_item);
    });

    factory
}

/// Installs the day-header factory that renders section starts as the
/// same centered date label the old separator rows used.
fn setup_day_headers(view: &gtk::ListView, timeline: &TimelineModel) {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, header| {
        let header = header
            .downcast_ref::<gtk::ListHeader>()
            .expect("Needs to be ListHeader");

        let label = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .css_classes(["service-message", "caption", "dimmed"])
            .margin_top(4)
            .margin_bottom(4)
            .build();
        header.set_child(Some(&label));
    });

    let timeline = timeline.clone();
    factory.connect_bind(move |_, header| {
        let header = header
            .downcast_ref::<gtk::ListHeader>()
            .expect("Needs to be ListHeader");

        let Some(label) = header.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(day) = timeline.row_day(header.start()) else {
            return;
        };

        label.set_label(&format_date_label(day));
    });

    view.set_header_factory(Some(&factory));
}

/// Owns the message list view and all pagination state for a single open chat.
pub(crate) struct ChatHistory {
    view: gtk::ListView,
    timeline: TimelineModel,
    is_group: bool,
    has_newer: bool,
    has_older: bool,
    quote_anchors: Rc<RefCell<HashMap<String, glib::WeakRef<gtk::Widget>>>>,
    identity_lid_to_pn: HashMap<String, String>,
    oldest_loaded_timestamp: Option<Timestamp>,
    newest_loaded_timestamp: Option<Timestamp>,
}

impl fmt::Debug for ChatHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatHistory")
            .field("rows", &self.timeline.len())
            .finish_non_exhaustive()
    }
}

impl ChatHistory {
    pub(crate) fn new(input: Option<relm4::Sender<ChatViewInput>>) -> Self {
        let timeline = TimelineModel::new();
        let selection = gtk::NoSelection::new(Some(timeline.clone()));
        let anchors = Rc::new(RefCell::new(HashMap::new()));
        let view = gtk::ListView::new(
            Some(selection),
            Some(build_row_factory(input, anchors.clone())),
        );
        setup_day_headers(&view, &timeline);

        Self {
            view,
            timeline,
            is_group: false,
            has_newer: false,
            has_older: false,
            quote_anchors: anchors,
            identity_lid_to_pn: HashMap::new(),
            oldest_loaded_timestamp: None,
            newest_loaded_timestamp: None,
        }
    }

    pub(crate) fn view(&self) -> &gtk::ListView {
        &self.view
    }

    /// Number of loaded rows.
    pub(crate) fn len(&self) -> u32 {
        self.timeline.len()
    }

    /// Whether the history is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.timeline.len() == 0
    }

    /// Records whether the open chat is a group, so author grouping can
    /// switch to the direction rule for private chats.
    pub(crate) fn set_group(&mut self, is_group: bool) {
        self.is_group = is_group;
    }

    /// Installs the LID→phone identity map a load resolved against,
    /// so author grouping treats one person's LID- and PN-keyed rows
    /// as the same sender. Cleared on reset like every other cursor.
    pub(crate) fn set_identities(&mut self, identities: HashMap<String, String>) {
        self.identity_lid_to_pn = identities;
    }

    /// The row index holding the message with this wire id.
    pub(crate) fn index_by_stanza(&self, stanza: &str) -> Option<u32> {
        for index in 0..self.len() {
            let Some(row) = self.get_row(index) else {
                continue;
            };

            if let ChatRow::Message { message, .. } = row
                && message.server_id == stanza
            {
                return Some(index);
            }
        }

        None
    }

    /// Toggles the quoted-row highlight. The class goes straight onto
    /// the row's widget — a model replace would re-anchor the view and
    /// drag the scroll position back, so the model stays untouched.
    pub(crate) fn set_row_highlight(&self, stanza: &str, on: bool) {
        let weak = self.quote_anchors.borrow().get(stanza).cloned();
        let Some(root) = weak.and_then(|anchor| anchor.upgrade()) else {
            self.quote_anchors.borrow_mut().remove(stanza);
            return;
        };

        if on {
            root.add_css_class("highlight");
        } else {
            root.remove_css_class("highlight");
        }
    }

    /// Reveals the row with the platform reveal scroll and glides the
    /// viewport over the segment it traveled, with no intermediate
    /// frame painted at the landing.
    pub(crate) fn scroll_to_row(&self, index: u32) {
        let Some(scroll) = self
            .view
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
        else {
            self.view
                .scroll_to(index, gtk::ListScrollFlags::FOCUS, None);
            return;
        };

        // The instant reveal computes the landing spot; the animation
        // just replays the segment from where the reader was. the
        // settled adjustment value is the target.
        let adj = scroll.vadjustment();
        let start = adj.value();
        self.view
            .scroll_to(index, gtk::ListScrollFlags::FOCUS, None);

        // Resetting to the start and gliding must happen while the
        // layout holds the reveal, before it paints; doing it on a
        // timer lets the landing frame flash first.
        let glide = move |adj: gtk::Adjustment| {
            let target = adj.value().clamp(
                adj.lower(),
                (adj.upper() - adj.page_size()).max(adj.lower()),
            );

            if (target - start).abs() < 1.0 {
                return;
            }

            adj.set_value(start);
            let value_target = adw::CallbackAnimationTarget::new(move |value| {
                adj.set_value(value);
            });

            let animation = adw::TimedAnimation::new(&scroll, start, target, 250, value_target);
            animation.set_easing(adw::Easing::EaseOutCubic);
            animation.play();
        };

        if (adj.value() - start).abs() > 0.5 {
            // The reveal applied synchronously; animate from here
            // before the frame paints.
            glide(adj);
            return;
        }

        // The reveal lands with the layout pass: catch its adjustment
        // change mid-layout, reset to the start before the paint, and
        // glide from there. The trigger is one-shot and expires, so a
        // reveal that never moves the viewport leaves nothing armed
        // against the reader's next manual scroll.
        let armed = Rc::new(Cell::new(true));
        let trigger = Rc::new(Cell::new(None::<glib::SignalHandlerId>));
        let armed_trigger = armed.clone();

        let id_ref = trigger.clone();
        let trigger_adj = adj.clone();
        let conn = adj.connect_value_changed(move |adj| {
            if !armed_trigger.replace(false) {
                return;
            }

            if let Some(id) = id_ref.take() {
                adj.disconnect(id);
            }

            glide(trigger_adj.clone());
        });
        trigger.set(Some(conn));

        let expiry_adj = adj;
        let expiry_id = trigger;
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if armed.replace(false)
                && let Some(id) = expiry_id.take()
            {
                expiry_adj.disconnect(id);
            }
        });
    }

    /// Live-updates the sender name of rows whose sender matches any
    /// of the jid forms. Day-preserving replaces leave the section
    /// layout untouched.
    pub(crate) fn update_sender_names(&self, forms: &[String], name: Option<&str>) {
        let bares: HashSet<String> = forms.iter().map(|form| bare_jid(form)).collect();
        for index in 0..self.len() {
            let Some(mut row) = self.get_row(index) else {
                continue;
            };
            let ChatRow::Message { message, .. } = &mut row else {
                continue;
            };

            if message.outgoing
                || !bares.contains(&bare_jid(&message.sender_jid))
                || message.sender_name.as_deref() == name
            {
                continue;
            }

            // This channel only ever carries contact-resolved names.
            message.sender_name = name.map(ToString::to_string);
            message.sender_push = false;
            self.replace_row(index, row, false);
        }
    }

    /// Live-updates the avatar of rows whose sender matches the jid.
    /// Day-preserving replaces leave the section layout untouched; the
    /// rebind re-reads the freshly cached avatar file.
    pub(crate) fn update_sender_avatars(&self, jid: &str) {
        let bare = bare_jid(jid);
        for index in 0..self.len() {
            let Some(mut row) = self.get_row(index) else {
                continue;
            };
            let ChatRow::Message { message, .. } = &mut row else {
                continue;
            };

            if message.outgoing || bare_jid(&message.sender_jid) != bare {
                continue;
            }

            self.replace_row(index, row, false);
        }
    }

    /// Swap in a fresh timeline: a per-Open reset instead of clearing one
    /// live model. `set_model` tears GTK's internal item-manager state down
    /// completely, so the next fill starts from a clean slate — mutating a
    /// cleared model can leave undisposed tile corpses behind (the
    /// `ensure_items` SIGABRT).
    pub(crate) fn reset(&mut self) {
        let timeline = TimelineModel::new();
        let selection = gtk::NoSelection::new(Some(timeline.clone()));
        self.view.set_model(Some(&selection));
        setup_day_headers(&self.view, &timeline);

        self.timeline = timeline;
        self.is_group = false;
        self.has_newer = false;
        self.has_older = false;
        self.identity_lid_to_pn = HashMap::new();
        self.oldest_loaded_timestamp = None;
        self.newest_loaded_timestamp = None;
        self.quote_anchors.borrow_mut().clear();
    }

    pub(crate) fn has_older(&self) -> bool {
        self.has_older
    }

    pub(crate) fn has_newer(&self) -> bool {
        self.has_newer
    }

    pub(crate) fn set_has_older(&mut self, has_older: bool) {
        self.has_older = has_older;
    }

    pub(crate) fn set_has_newer(&mut self, has_newer: bool) {
        self.has_newer = has_newer;
    }

    pub(crate) fn oldest_timestamp(&self) -> Option<Timestamp> {
        self.oldest_loaded_timestamp
    }

    pub(crate) fn newest_timestamp(&self) -> Option<Timestamp> {
        self.newest_loaded_timestamp
    }

    /// Fill the history with an initial batch of messages, newest first.
    pub(crate) fn fill(&mut self, messages: &[ChatMessage]) -> bool {
        self.view.set_opacity(0.0);

        if messages.is_empty() {
            return false;
        }

        // Track the oldest loaded timestamp for pagination.
        if let Some(oldest) = messages.last() {
            self.oldest_loaded_timestamp = Some(oldest.timestamp);
        }

        // Track the newest loaded timestamp for downward pagination.
        if let Some(newest) = messages.first() {
            self.newest_loaded_timestamp = Some(newest.timestamp);
        }

        let unread_boundary = messages.iter().rposition(|msg| {
            !msg.outgoing && matches!(msg.status, MessageStatus::Sent | MessageStatus::Delivered)
        });

        let tz = TimeZone::system();
        let mut rows: Vec<(ChatRow, Date)> = Vec::with_capacity(messages.len() + 1);
        let mut in_unread = false;
        let mut prev_date: Option<Date> = None;
        // The chronologically previous (older) message, already appended.
        let mut prev_msg: Option<&ChatMessage> = None;

        for (i, msg) in messages.iter().enumerate().rev() {
            let msg_date = msg.timestamp.to_zoned(tz.clone()).date();
            let date_break = prev_date != Some(msg_date);

            let divider_pushed = if Some(i) == unread_boundary {
                rows.push((ChatRow::UnreadDivider, msg_date));
                in_unread = true;
                true
            } else {
                false
            };

            let first = match prev_msg {
                Some(prev) if !date_break => {
                    !same_group(prev, msg, &self.identity_lid_to_pn, self.is_group)
                }
                _ => true,
            };
            let first_of_segment = in_unread && date_break && !divider_pushed;

            // The chronologically next (newer) message.
            let next = (i > 0).then(|| &messages[i - 1]);
            let next_date = next.map(|n| n.timestamp.to_zoned(tz.clone()).date());
            let last = next_date.is_none_or(|nd| nd != msg_date)
                || next
                    .is_some_and(|n| !same_group(msg, n, &self.identity_lid_to_pn, self.is_group));
            let last_of_segment = in_unread && next_date != Some(msg_date);

            rows.push((
                ChatRow::Message {
                    last,
                    first,
                    unread: in_unread,
                    message: msg.clone(),

                    last_of_segment,
                    first_of_segment,
                },
                msg_date,
            ));

            prev_date = Some(msg_date);
            prev_msg = Some(msg);
        }

        self.timeline.append_batch(rows);

        true
    }

    /// Append a single live message to the bottom. Updates only the newest
    /// cursor. `at_bottom` tells whether the user is watching the newest
    /// messages, in which case an arrival is seen live and never counts
    /// as unread.
    pub(crate) fn append_live(&mut self, message: ChatMessage, at_bottom: bool) {
        // A reply drains the band: the divider demotes before the
        // message appends below where it sat.
        if message.outgoing {
            self.demote_unread_band();
        }

        let msg_date = message_day(&message);
        let in_unread = self
            .timeline
            .any_row(|row| matches!(row, ChatRow::UnreadDivider));
        let unread = !message.outgoing && in_unread && !at_bottom;

        let old_bottom_idx = self.timeline.len().checked_sub(1);
        let old_bottom_row = old_bottom_idx.and_then(|idx| self.timeline.row(idx));

        let bottom_group = match &old_bottom_row {
            Some(ChatRow::Message { message: prev, .. }) => Some((
                message_day(prev),
                same_group(prev, &message, &self.identity_lid_to_pn, self.is_group),
            )),
            _ => None,
        };
        let date_break = bottom_group.is_none_or(|(day, _)| day != msg_date);
        let boundary_group = bottom_group.is_some_and(|(day, grouped)| day == msg_date && grouped);

        if in_unread && !date_break {
            self.demote_band_tail();
        }

        // The old bottom row is absorbed by the boundary group: it no
        // longer renders the group tail.
        if boundary_group
            && let (
                Some(idx),
                Some(ChatRow::Message {
                    last: _,
                    first,
                    unread: prev_unread,
                    message: prev,

                    last_of_segment: prev_last_of_segment,
                    first_of_segment: prev_first_of_segment,
                }),
            ) = (old_bottom_idx, old_bottom_row)
        {
            let day = message_day(&prev);
            let updated = ChatRow::Message {
                last: false,
                first,
                unread: prev_unread,
                message: prev,

                last_of_segment: prev_last_of_segment,
                first_of_segment: prev_first_of_segment,
            };
            self.timeline.replace(idx, updated, day);
        }

        self.newest_loaded_timestamp = Some(message.timestamp);

        let first = !boundary_group;
        let first_of_segment = in_unread && date_break;
        self.timeline.append_batch(vec![(
            ChatRow::Message {
                last: true,
                first,
                unread,
                message,

                last_of_segment: in_unread,
                first_of_segment,
            },
            msg_date,
        )]);
    }

    /// Prepend a batch of older messages to the top, newest first.
    pub(crate) fn prepend_messages(&mut self, messages: &[ChatMessage]) {
        if messages.is_empty() {
            return;
        }

        // Update the oldest loaded timestamp cursor.
        if let Some(oldest) = messages.last() {
            self.oldest_loaded_timestamp = Some(oldest.timestamp);
        }

        let newest = &messages[0];
        let old_top_row = self
            .timeline
            .position_row(|row| matches!(row, ChatRow::Message { .. }))
            .and_then(|idx| self.timeline.row(idx));

        let boundary_group = match &old_top_row {
            Some(ChatRow::Message { message: prev, .. }) => {
                same_group(prev, newest, &self.identity_lid_to_pn, self.is_group)
            }
            _ => false,
        };

        let tz = TimeZone::system();
        let mut rows: Vec<(ChatRow, Date)> = Vec::with_capacity(messages.len());
        // Chronologically previous (older) state within the batch.
        let mut prev_date: Option<Date> = None;
        let mut prev_msg: Option<&ChatMessage> = None;

        for (i, msg) in messages.iter().enumerate().rev() {
            let msg_date = msg.timestamp.to_zoned(tz.clone()).date();

            let first = match prev_msg {
                Some(prev) if prev_date == Some(msg_date) => {
                    !same_group(prev, msg, &self.identity_lid_to_pn, self.is_group)
                }
                _ => true,
            };
            let last = if i == 0 {
                !boundary_group
            } else {
                let next = &messages[i - 1];
                let next_date = next.timestamp.to_zoned(tz.clone()).date();
                next_date != msg_date
                    || !same_group(msg, next, &self.identity_lid_to_pn, self.is_group)
            };

            rows.push((
                ChatRow::Message {
                    last,
                    first,
                    unread: false,
                    message: msg.clone(),

                    last_of_segment: false,
                    first_of_segment: false,
                },
                msg_date,
            ));

            prev_date = Some(msg_date);
            prev_msg = Some(msg);
        }

        self.timeline.prepend_batch(rows);
    }

    /// Append a batch of newer messages to the bottom, oldest first.
    pub(crate) fn append_newer(&mut self, messages: &[ChatMessage]) -> bool {
        if messages.is_empty() {
            return false;
        }

        let tz = TimeZone::system();
        let oldest_batch = &messages[0];
        let oldest_batch_date = oldest_batch.timestamp.to_zoned(tz.clone()).date();
        let in_unread = self
            .timeline
            .any_row(|row| matches!(row, ChatRow::UnreadDivider));

        let old_bottom_idx = self.timeline.len().checked_sub(1);
        let old_bottom_row = old_bottom_idx.and_then(|idx| self.timeline.row(idx));

        let bottom_group = match &old_bottom_row {
            Some(ChatRow::Message { message: prev, .. }) => Some((
                message_day(prev),
                same_group(prev, oldest_batch, &self.identity_lid_to_pn, self.is_group),
            )),
            _ => None,
        };
        let boundary_group =
            bottom_group.is_some_and(|(day, grouped)| day == oldest_batch_date && grouped);

        if in_unread && bottom_group.is_some_and(|(day, _)| day == oldest_batch_date) {
            self.demote_band_tail();
        }

        let mut rows: Vec<(ChatRow, Date)> = Vec::with_capacity(messages.len());
        // Chronologically previous (older) state; seeded from the old
        // bottom row so the batch groups with it.
        let mut prev_date: Option<Date> = bottom_group.map(|(day, _)| day);
        let mut prev_msg: Option<&ChatMessage> = None;

        for (i, msg) in messages.iter().enumerate() {
            let msg_date = msg.timestamp.to_zoned(tz.clone()).date();
            let date_break = prev_date != Some(msg_date);

            let first = if i == 0 {
                !boundary_group
            } else if let Some(prev) = prev_msg {
                !same_group(prev, msg, &self.identity_lid_to_pn, self.is_group)
            } else {
                true
            };
            let first_of_segment = in_unread && date_break;

            // The chronologically next (newer) message.
            let next = messages.get(i + 1);
            let next_date = next.map(|n| n.timestamp.to_zoned(tz.clone()).date());
            let last = next_date.is_none_or(|nd| nd != msg_date)
                || next
                    .is_some_and(|n| !same_group(msg, n, &self.identity_lid_to_pn, self.is_group));
            let last_of_segment = in_unread && next_date != Some(msg_date);

            rows.push((
                ChatRow::Message {
                    last,
                    first,
                    unread: in_unread,
                    message: msg.clone(),

                    last_of_segment,
                    first_of_segment,
                },
                msg_date,
            ));

            prev_date = Some(msg_date);
            prev_msg = Some(msg);
        }

        self.timeline.append_batch(rows);

        // The old bottom row is absorbed by the boundary group: it no
        // longer renders the group tail.
        if boundary_group
            && let (
                Some(idx),
                Some(ChatRow::Message {
                    last: _,
                    first,
                    unread: prev_unread,
                    message: prev,

                    last_of_segment: prev_last_of_segment,
                    first_of_segment: prev_first_of_segment,
                }),
            ) = (old_bottom_idx, old_bottom_row)
        {
            let updated = ChatRow::Message {
                last: false,
                first,
                unread: prev_unread,
                message: prev.clone(),

                last_of_segment: prev_last_of_segment,
                first_of_segment: prev_first_of_segment,
            };
            self.timeline.replace(idx, updated, message_day(&prev));
        }

        // Update the newest loaded timestamp cursor.
        if let Some(newest) = messages.last() {
            self.newest_loaded_timestamp = Some(newest.timestamp);
        }

        true
    }

    /// Find the index of the first message row matching `predicate`.
    pub(crate) fn find_message_index(
        &self,
        predicate: impl Fn(&ChatMessage) -> bool,
    ) -> Option<u32> {
        self.timeline.position_row(|row| match row {
            ChatRow::Message { message, .. } => predicate(message),
            _ => false,
        })
    }

    /// Get a cloned copy of the row at `index`.
    pub(crate) fn get_row(&self, index: u32) -> Option<ChatRow> {
        self.timeline.row(index)
    }

    /// Whether any message row is still marked unread.
    pub(crate) fn has_unread_messages(&self) -> bool {
        self.timeline
            .any_row(|row| matches!(row, ChatRow::Message { unread: true, .. }))
    }

    /// Index of the unread divider row, when the band is present.
    pub(crate) fn unread_divider_index(&self) -> Option<u32> {
        self.timeline
            .position_row(|row| matches!(row, ChatRow::UnreadDivider))
    }

    /// Scrolls to the bottom (forcing full layout), then up so the row at
    /// `target` is revealed at the top edge. Used to open a chat at the
    /// unread divider, with earlier messages kept above as context.
    pub(crate) fn scroll_to_unread_boundary(&self, target: u32) {
        fn scroll_to_row(view: &gtk::ListView, index: u32) {
            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            view.scroll_to(index, gtk::ListScrollFlags::NONE, Some(info));
        }

        fn scroll_to_bottom(view: &gtk::ListView) -> bool {
            let count = view.model().map_or(0, |model| model.n_items());
            if count == 0 {
                return true;
            }

            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            view.scroll_to(count - 1, gtk::ListScrollFlags::FOCUS, Some(info));

            view.vadjustment()
                .is_some_and(|adj| adj.value() + adj.page_size() >= adj.upper() - 25.0)
        }

        let view = self.view.clone();
        let mut attempts = 0;
        let mut pivoted = false;
        let mut last_upper = None::<f64>;
        let mut last_value = None::<f64>;

        glib::timeout_add_local(Duration::from_millis(16), move || {
            attempts += 1;

            if pivoted {
                let value = view.vadjustment().map(|adj| adj.value());
                let stable = match (value, last_value) {
                    (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
                    _ => false,
                };
                last_value = value;

                if stable || attempts >= 40 {
                    view.set_opacity(1.0);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            } else {
                let upper = view.vadjustment().map(|adj| adj.upper());
                let settled = scroll_to_bottom(&view);
                let stable = match (upper, last_upper) {
                    (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
                    _ => false,
                };
                last_upper = upper;

                if (settled && stable) || attempts >= 40 {
                    scroll_to_row(&view, target);
                    pivoted = true;
                    attempts = 0;
                    last_value = None;
                }

                glib::ControlFlow::Continue
            }
        });
    }

    /// Remove the row at `index` and re-insert a replacement, preserving scroll.
    pub(crate) fn replace_row(&self, index: u32, new_row: ChatRow, at_bottom: bool) {
        let adj = self.view.vadjustment();
        let saved_scroll = if at_bottom {
            None
        } else {
            adj.as_ref().map(AdjustmentExt::value)
        };

        let day = match &new_row {
            ChatRow::Message { message, .. } => message_day(message),
            _ => self.timeline.row_day(index).unwrap_or_default(),
        };
        self.timeline.replace(index, new_row, day);

        if let (Some(adj), Some(value)) = (adj, saved_scroll) {
            glib::idle_add_local_once(move || adj.set_value(value));
        }
    }

    /// Current adjustment (value, upper), for scroll diagnostics.
    pub(crate) fn scroll_position(&self) -> (f64, f64) {
        self.view
            .vadjustment()
            .map_or((0.0, 0.0), |adj| (adj.value(), adj.upper()))
    }

    /// Whether the viewport is truly touching the bottom edge, unlike
    /// the generous anchor-band check used for streaming which spans
    /// roughly two rows.
    pub(crate) fn truly_at_bottom(&self) -> bool {
        self.view
            .vadjustment()
            .is_some_and(|adj| adj.value() + adj.page_size() >= adj.upper() - 2.0)
    }

    /// Play the landing as a short ease-out glide from the current
    /// scroll position to the bottom of the content. Never rewinds:
    /// the value only moves toward the target, so the destination can
    /// never flash before the slide.
    fn play_glide(view: &gtk::ListView, adj: &gtk::Adjustment, gliding: Rc<Cell<bool>>) {
        let start = adj.value();
        let target = (adj.upper() - adj.page_size()).max(adj.lower());
        let travel = (target - start).abs();
        if travel < 2.0 {
            return;
        }

        // Duration follows the distance: a row of travel is a snap,
        // a couple of screens plays out fully without ever feeling
        // like a page-turn.
        // Clamped into a millisecond domain where truncation is
        // provably value-less.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let truncated = (travel as i64).max(0) as u64;
        let duration = (120u64 + truncated.saturating_mul(8) / 100).min(260);
        let duration = u32::try_from(duration).unwrap_or(260);

        gliding.set(true);

        let animation = adw::TimedAnimation::new(
            view,
            start,
            target,
            duration,
            adw::CallbackAnimationTarget::new({
                let adj = adj.clone();
                move |value| adj.set_value(value)
            }),
        );
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.connect_done(move |_| gliding.set(false));
        animation.play();
    }

    /// Scroll the list view to the bottom (last row). Landings within
    /// a couple of screens replay as a short ease-out glide; longer
    /// ones snap instantly.
    pub(crate) fn scroll_to_bottom(&self, on_settled: impl FnOnce() + 'static) {
        fn scroll(view: &gtk::ListView) -> bool {
            let count = view.model().map_or(0, |model| model.n_items());
            if count == 0 {
                return true;
            }

            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            view.scroll_to(count - 1, gtk::ListScrollFlags::FOCUS, Some(info));

            view.vadjustment()
                .is_some_and(|adj| adj.value() + adj.page_size() >= adj.upper() - 25.0)
        }

        /// Longer travels (past a couple of screens) snap instantly:
        /// easing across many rows reads as a zoom, not as a landing.
        fn glide_budget(adj: &gtk::Adjustment) -> f64 {
            2.5 * adj.page_size()
        }

        let view = self.view.clone();
        let mut attempts = 0;
        let mut last_upper: Option<f64> = None;

        let gliding = Rc::new(Cell::new(false));
        let glide_trigger = Rc::new(Cell::new(None));
        let trigger_armed = Rc::new(Cell::new(false));

        // Compose the landing on idle, past the model's own mutation
        // pass, and measure it from the adjustment alone.
        glib::idle_add_local_once({
            let view = view.clone();
            let gliding = gliding.clone();
            let glide_trigger = glide_trigger.clone();
            let trigger_armed = trigger_armed.clone();

            move || {
                let Some(adj) = view.vadjustment() else {
                    return;
                };

                let travel = (adj.upper() - adj.page_size()).max(adj.lower()) - adj.value();
                if travel > glide_budget(&adj) {
                    scroll(&view);
                    return;
                }

                if travel > 2.0 {
                    Self::play_glide(&view, &adj, gliding.clone());
                    return;
                }

                // Essentially at the bottom, or the new row is not laid
                // out yet: wait for the growth that brings it, and let
                // the trigger — not the loop — drive that landing.
                let handler = {
                    let view = view.clone();
                    let gliding = gliding.clone();
                    let glide_trigger = glide_trigger.clone();
                    let trigger_armed = trigger_armed.clone();

                    adj.connect_value_changed(move |adj| {
                        if glide_trigger.take().is_none() {
                            return;
                        }
                        trigger_armed.set(false);

                        let travel = (adj.upper() - adj.page_size()).max(adj.lower()) - adj.value();
                        if travel > 2.0 && travel <= glide_budget(adj) {
                            Self::play_glide(&view, adj, gliding.clone());
                        }
                    })
                };

                trigger_armed.set(true);
                glide_trigger.set(Some(handler));
            }
        });

        let mut on_settled = Some(on_settled);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let armed = gliding.get() || trigger_armed.get();

            attempts += 1;

            let Some(adj) = view.vadjustment() else {
                if attempts >= 40 {
                    view.set_opacity(1.0);
                    if let Some(on_settled) = on_settled.take() {
                        on_settled();
                    }
                    return glib::ControlFlow::Break;
                }

                return glib::ControlFlow::Continue;
            };

            // Re-anchor only when the content actually grew: re-jumping
            // to the last row on every tick fights the layout and is
            // what the scrollbar shows as a fast up-and-down dance. A
            // playing or armed glide owns the growth instead.
            let upper_now = adj.upper();
            let moved = last_upper.is_none_or(|last| last.to_bits() != upper_now.to_bits());
            let settled = if moved && !armed {
                scroll(&view)
            } else {
                adj.value() + adj.page_size() >= upper_now - 25.0
            };
            last_upper = Some(upper_now);

            if (settled && !moved) || attempts >= 40 {
                if let Some(trigger) = glide_trigger.take() {
                    adj.disconnect(trigger);
                }
                trigger_armed.set(false);

                // A landing that never materialized still owes the
                // bottom one instant hop.
                if !settled {
                    scroll(&view);
                }

                view.set_opacity(1.0);

                if let Some(on_settled) = on_settled.take() {
                    on_settled();
                }

                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// A sent message drains the band: the unread divider goes away
    /// and rows built as unread stop rendering the band styling.
    fn demote_unread_band(&self) {
        let Some(divider) = self
            .timeline
            .position_row(|row| matches!(row, ChatRow::UnreadDivider))
        else {
            return;
        };

        for idx in (divider + 1)..self.timeline.len() {
            let Some(ChatRow::Message {
                last,
                first,
                unread: true,
                message,
                last_of_segment,
                first_of_segment,
            }) = self.timeline.row(idx)
            else {
                continue;
            };

            let day = message_day(&message);
            self.timeline.replace(
                idx,
                ChatRow::Message {
                    last,
                    first,
                    unread: false,
                    message,
                    last_of_segment,
                    first_of_segment,
                },
                day,
            );
        }

        self.timeline.remove(divider);
    }

    /// The last unread message stops being the band tail once more of
    /// the band arrives below it.
    fn demote_band_tail(&self) {
        let Some(idx) = self.timeline.len().checked_sub(1) else {
            return;
        };

        let Some(ChatRow::Message {
            last,
            first,
            unread: true,
            message,
            last_of_segment: _,
            first_of_segment,
        }) = self.timeline.last_row()
        else {
            return;
        };

        let day = message_day(&message);
        let updated = ChatRow::Message {
            last,
            first,
            unread: true,
            message,

            last_of_segment: false,
            first_of_segment,
        };
        self.timeline.replace(idx, updated, day);
    }
}

#[cfg(test)]
mod timeline_tests {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    use gtk::{glib, prelude::*};
    use indexmap::IndexMap;
    use jiff::Timestamp;
    use relm4::gtk;
    use uuid::Uuid;

    use super::{ChatHistory, ChatRow};
    use crate::state::{ChatMessage, MessageStatus};

    /// A newest-first page of synthetic messages, `count` long, each
    /// spaced `spacing` seconds apart. The default callers use four
    /// hours so several calendar days are covered; the same-day caller
    /// uses one minute so the whole page shares a single day section.
    /// With `unread` set, the newest messages are unread, so the
    /// divider lands a handful of rows above the bottom — the shape
    /// real chats open in.
    fn page(count: usize, unread: bool, spacing: i64) -> Vec<ChatMessage> {
        let now = Timestamp::now().as_second();
        (0..count)
            .map(|i| ChatMessage {
                media: None,
                quote: None,
                status: if unread && i < 6 && i % 4 == 1 {
                    MessageStatus::Delivered
                } else {
                    MessageStatus::Read
                },
                content: format!("message {i}"),
                chat_jid: "test@g.us".to_string(),
                local_id: Uuid::new_v4(),
                outgoing: i % 4 != 1,
                reactions: IndexMap::new(),
                sender_jid: "1000000000@s.whatsapp.net".to_string(),
                server_id: format!("srv-{i}"),
                timestamp: Timestamp::from_second(now - (i as i64) * spacing)
                    .expect("timestamp fits"),
                sender_name: None,
                sender_push: false,
            })
            .collect::<Vec<_>>()
    }

    /// Pumps the default main context for `ms`, giving GTK time to run
    /// frames — and the list item manager time to walk the model.
    fn pump(ms: u64) {
        let context = glib::MainContext::default();
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            while context.iteration(false) {}
            thread::sleep(Duration::from_millis(16));
        }

        while context.iteration(false) {}
    }

    /// Replays the crash choreography from the field logs against the real
    /// widget machinery: a chat filled short then open-clear-refill with a
    /// rendered empty gap; then, from a fresh clear, the streamed fill with
    /// its bottom-scroll loop running while the older tail lands on top.
    /// The abort under investigation fires inside `ensure_items` on such
    /// refills in the real app.
    #[test]
    #[ignore = "needs a running display; drives GTK's list item manager"]
    fn clear_refill_and_streamed_fill_survive_frames() {
        gtk::init().expect("gtk needs a display");

        let mut history = ChatHistory::new(None);
        let window = gtk::Window::new();
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(history.view()));
        window.set_child(Some(&scrolled));
        window.set_default_size(400, 600);
        window.present();
        pump(300);

        // Chat A fills a short page and renders.
        history.fill(&page(18, false, 14_400));
        assert_eq!(history.len(), 18);
        pump(750);

        // Opening the next chat clears the timeline back to zero.
        history.reset();
        assert_eq!(history.len(), 0);
        pump(500);

        // Chat B opens with unread: fill, divider scroll, and status ticks
        // reshaping rows while that scroll loop is still running.
        history.fill(&page(40, true, 14_400));
        assert_eq!(history.len(), 41);
        if let Some(divider) = history.unread_divider_index() {
            assert!(divider > 4, "divider must sit past the unread band");
            history.scroll_to_unread_boundary(divider - 4);
        }
        pump(100);

        // Receipts tick statuses on the just-opened page while the two
        // phase unread scroll is still settling.
        for _ in 0..6 {
            if let Some(row) = history.get_row(0)
                && let ChatRow::Message { message, .. } = &row
                && message.status != MessageStatus::Read
            {
                let local_id = message.local_id;
                let index = history
                    .find_message_index(|message| message.local_id == local_id)
                    .expect("row must be findable");

                let mut row = history.get_row(index).expect("row must exist");
                if let ChatRow::Message { message, .. } = &mut row {
                    message.status = MessageStatus::Read;
                }

                history.replace_row(index, row, false);
            }
            pump(50);
        }
        pump(900);

        // Back out, then stream chat C: prefix first, scroll loop still
        // spinning while the older tail lands on top. Unread pages don't
        // stream in production, so this page is all-read.
        history.reset();
        assert_eq!(history.len(), 0);
        pump(400);

        let page_c = page(40, false, 14_400);
        let (prefix, tail) = page_c.split_at(20);
        history.fill(prefix);
        assert_eq!(history.len(), 20);
        history.scroll_to_bottom(|| {});
        pump(100);

        history.prepend_messages(tail);
        assert_eq!(history.len(), 40);
        pump(900);

        history.reset();
        assert_eq!(history.len(), 0);
        pump(300);

        // Chat D replays the machine-gun open the field crash came from:
        // open, fill, and the streamed tail all land inside one main
        // context dispatch, with no frame in between for the list item
        // manager to settle its tile bookkeeping.
        history.fill(prefix);
        assert_eq!(history.len(), 20);
        history.scroll_to_bottom(|| {});
        history.prepend_messages(tail);
        assert_eq!(history.len(), 40);
        history.scroll_to_bottom(|| {});
        history.fill(&page(40, true, 14_400));
        pump(900);

        // Chat E isolates the same-day merge: a page dense enough that
        // every message shares one calendar day, so the streamed tail
        // prepends into the existing day section instead of pushing a
        // fresh header through it. If the old top header's stale range
        // never gets repaired, this is where the field abort fires.
        history.reset();
        assert_eq!(history.len(), 0);
        pump(300);

        let page_e = page(40, false, 60);
        let (prefix_e, tail_e) = page_e.split_at(20);
        history.fill(prefix_e);
        assert_eq!(history.len(), 20);
        history.scroll_to_bottom(|| {});
        pump(100);

        history.prepend_messages(tail_e);
        assert_eq!(history.len(), 40);
        history.scroll_to_bottom(|| {});

        pump(100);
        // Walk the whole list: tracking the seam positions is what makes
        // the item manager hit a stale tile and abort in the field.
        scrolled.vadjustment().set_value(0.0);
        pump(900);

        history.reset();
        assert_eq!(history.len(), 0);
        pump(300);
    }
}
