use std::collections::VecDeque;

use adw::prelude::*;
use chrono::{Local, NaiveDate};
use gtk::{gio, glib};
use relm4::{prelude::*, typed_view::list::TypedListView};

use super::rows::ChatRow;
use crate::state::ChatMessage;

fn same_group(a: &ChatMessage, b: &ChatMessage) -> bool {
    a.outgoing == b.outgoing && a.sender_jid == b.sender_jid
}

fn build_prepend_rows(
    messages: &[ChatMessage],
    boundary_group: bool,
) -> (Vec<ChatRow>, Vec<RowMetadata>) {
    let mut rows = Vec::with_capacity(messages.len());
    let mut metas = Vec::with_capacity(messages.len());
    let mut prev_date: Option<NaiveDate> = None;
    let mut prev_msg: Option<&ChatMessage> = None;

    // Reverse messages to get chronological order for prepending.
    for (i, msg) in messages.iter().enumerate().rev() {
        let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

        let first = if prev_date == Some(msg_date)
            && let Some(prev) = prev_msg
        {
            !same_group(prev, msg)
        } else {
            true
        };

        // Insert a date separator if the date changed.
        if prev_date != Some(msg_date) {
            rows.push(ChatRow::DateSeparator(msg_date));
            metas.push(RowMetadata::Separator(msg_date));
            prev_date = Some(msg_date);
        }

        let last = if i == 0 {
            !boundary_group
        } else {
            let next = &messages[i - 1];
            let next_date = next.timestamp.with_timezone(&Local).date_naive();
            next_date != msg_date || !same_group(msg, next)
        };

        let ts = msg.timestamp.timestamp();
        rows.push(ChatRow::Message {
            last,
            first,
            message: msg.clone(),
        });
        metas.push(RowMetadata::Message(ts));
        prev_msg = Some(msg);
    }

    (rows, metas)
}

/// Metadata for a single row in the message list, used for cursor tracking
/// when trimming rows during bidirectional pagination.
#[derive(Clone, Debug)]
pub(crate) enum RowMetadata {
    /// A message row, with its Unix timestamp.
    Message(i64),
    /// A date separator row.
    Separator(NaiveDate),
}

/// Owns the message list view and all pagination state for a single open chat.
#[derive(Debug)]
pub(crate) struct ChatHistory {
    /// The list view wrapper holding all chat rows.
    list: TypedListView<ChatRow, gtk::NoSelection>,
    /// Metadata mirror of `list`, used for cursor tracking.
    row_metadata: VecDeque<RowMetadata>,

    /// Whether there might be older messages to load (above the current top).
    has_older: bool,
    /// Whether there might be newer messages to load (below the current bottom).
    has_newer: bool,

    /// Timestamp of the oldest loaded message (top cursor).
    oldest_loaded_timestamp: Option<i64>,
    /// Timestamp of the newest loaded message (bottom cursor).
    newest_loaded_timestamp: Option<i64>,
    /// Date of the first displayed message (top), for prepend separator logic.
    first_message_date: Option<NaiveDate>,
    /// Date of the last appended message (bottom), for append separator logic.
    last_message_date: Option<NaiveDate>,
}

impl ChatHistory {
    /// Create an empty `ChatHistory`.
    pub(crate) fn new() -> Self {
        Self {
            list: TypedListView::new(),
            row_metadata: VecDeque::new(),
            has_older: false,
            has_newer: false,
            oldest_loaded_timestamp: None,
            newest_loaded_timestamp: None,
            first_message_date: None,
            last_message_date: None,
        }
    }

    /// Borrow the underlying list view.
    pub(crate) fn view(&self) -> &TypedListView<ChatRow, gtk::NoSelection> {
        &self.list
    }

    /// Number of loaded rows.
    pub(crate) fn len(&self) -> u32 {
        self.list.len()
    }

    /// Whether the history is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Clear all rows and reset all cursors.
    pub(crate) fn clear(&mut self) {
        self.list.clear();
        self.row_metadata.clear();
        self.has_older = false;
        self.has_newer = false;
        self.oldest_loaded_timestamp = None;
        self.newest_loaded_timestamp = None;
        self.first_message_date = None;
        self.last_message_date = None;
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

    pub(crate) fn oldest_timestamp(&self) -> Option<i64> {
        self.oldest_loaded_timestamp
    }

    pub(crate) fn newest_timestamp(&self) -> Option<i64> {
        self.newest_loaded_timestamp
    }

    /// Fill the history with an initial batch of messages.
    pub(crate) fn fill(&mut self, messages: &[ChatMessage]) -> bool {
        if messages.is_empty() {
            return false;
        }

        // Track the oldest loaded timestamp for pagination.
        if let Some(oldest) = messages.last() {
            self.oldest_loaded_timestamp = Some(oldest.timestamp.timestamp());
        }

        // Track the newest loaded timestamp for downward pagination.
        if let Some(newest) = messages.first() {
            self.newest_loaded_timestamp = Some(newest.timestamp.timestamp());
        }

        for (i, msg) in messages.iter().enumerate().rev() {
            // Convert to local date for separator comparison.
            let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

            // Insert a date separator if the date changed.
            if self.last_message_date != Some(msg_date) {
                self.list.append(ChatRow::DateSeparator(msg_date));
                self.row_metadata
                    .push_back(RowMetadata::Separator(msg_date));
                self.last_message_date = Some(msg_date);
            }

            // Track the first message date for prepend separators.
            if self.first_message_date.is_none() {
                self.first_message_date = Some(msg_date);
            }

            let first = match (self.row_metadata.back(), messages.get(i + 1)) {
                (Some(RowMetadata::Message(_)), Some(prev)) => !same_group(prev, msg),
                _ => true,
            };
            let last = if i == 0 {
                true
            } else {
                let next = &messages[i - 1];
                let next_date = next.timestamp.with_timezone(&Local).date_naive();
                next_date != msg_date || !same_group(msg, next)
            };

            let ts = msg.timestamp.timestamp();
            self.list.append(ChatRow::Message {
                last,
                first,
                message: msg.clone(),
            });
            self.row_metadata.push_back(RowMetadata::Message(ts));
        }

        true
    }

    /// Append a single live message to the bottom. Updates only the newest
    /// cursor.
    pub(crate) fn append_live(&mut self, message: ChatMessage) {
        // Convert to local date for separator comparison.
        let msg_date = message.timestamp.with_timezone(&Local).date_naive();

        let mut first = true;
        if self.last_message_date == Some(msg_date)
            && matches!(self.row_metadata.back(), Some(RowMetadata::Message(_)))
        {
            let idx = self.list.len() - 1;
            if let Some(ChatRow::Message {
                last: _,
                first: prev_first,
                message: prev,
            }) = self.get_row(idx)
                && same_group(&prev, &message)
            {
                first = false;
                let updated = ChatRow::Message {
                    last: false,
                    first: prev_first,
                    message: prev,
                };
                let object = glib::BoxedAnyObject::new(updated);
                self.store().splice(idx, 1, &[object]);
            }
        }

        // Insert a date separator if the date changed.
        if self.last_message_date != Some(msg_date) {
            self.list.append(ChatRow::DateSeparator(msg_date));
            self.row_metadata
                .push_back(RowMetadata::Separator(msg_date));
            self.last_message_date = Some(msg_date);
        }

        // Update newest loaded timestamp to this message.
        let ts = message.timestamp.timestamp();
        self.newest_loaded_timestamp = Some(ts);

        self.list.append(ChatRow::Message {
            last: true,
            first,
            message,
        });
        self.row_metadata.push_back(RowMetadata::Message(ts));
    }

    /// Prepend a batch of older messages to the top; reversed internally.
    pub(crate) fn prepend_messages(&mut self, messages: &[ChatMessage]) {
        if messages.is_empty() {
            return;
        }

        // Update the oldest loaded timestamp cursor.
        if let Some(oldest) = messages.last() {
            self.oldest_loaded_timestamp = Some(oldest.timestamp.timestamp());
        }

        let newest = &messages[0];
        let newest_date = newest.timestamp.with_timezone(&Local).date_naive();

        let old_top_idx = self
            .row_metadata
            .iter()
            .position(|m| matches!(m, RowMetadata::Message(_)))
            .and_then(|i| u32::try_from(i).ok());
        let old_top_row = old_top_idx.and_then(|i| self.get_row(i));
        let front_is_message = matches!(self.row_metadata.front(), Some(RowMetadata::Message(_)));

        // Remove duplicate date separator if the last prepended date matches
        // the first existing date separator.
        let remove_old_separator = matches!(
            self.row_metadata.front(),
            Some(RowMetadata::Separator(date)) if *date == newest_date
        );

        let boundary_group = match &old_top_row {
            Some(ChatRow::Message { message: prev, .. }) => {
                (front_is_message || remove_old_separator) && same_group(prev, newest)
            }
            _ => false,
        };

        let (rows, mut metas) = build_prepend_rows(messages, boundary_group);

        let objects: Vec<glib::BoxedAnyObject> =
            rows.into_iter().map(glib::BoxedAnyObject::new).collect();
        if let Some(scrolled_window) = self
            .list
            .view
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            scrolled_window.set_kinetic_scrolling(false);
            scrolled_window.set_kinetic_scrolling(true);
        }
        if let Some(anchor) = self
            .row_metadata
            .iter()
            .position(|m| matches!(m, RowMetadata::Message(_)))
            .and_then(|i| u32::try_from(i).ok())
        {
            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            self.list
                .view
                .scroll_to(anchor, gtk::ListScrollFlags::NONE, Some(info));
        }
        self.store()
            .splice(0, u32::from(remove_old_separator), &objects);

        if remove_old_separator {
            self.row_metadata.pop_front();
        }
        metas.reverse();
        for meta in metas {
            self.row_metadata.push_front(meta);
        }

        if boundary_group
            && let (
                Ok(inserted),
                Some(ChatRow::Message {
                    last,
                    first: _,
                    message,
                }),
            ) = (u32::try_from(objects.len()), old_top_row)
        {
            let updated = ChatRow::Message {
                last,
                first: false,
                message,
            };
            let object = glib::BoxedAnyObject::new(updated);
            self.store().splice(inserted, 1, &[object]);
        }

        // Update first_message_date to the oldest prepended message's date.
        if let Some(oldest_msg) = messages.last() {
            self.first_message_date = Some(oldest_msg.timestamp.with_timezone(&Local).date_naive());
        }
    }

    /// Append a batch of newer messages to the bottom; iterated forward.
    pub(crate) fn append_newer(&mut self, messages: &[ChatMessage]) -> bool {
        if messages.is_empty() {
            return false;
        }

        let oldest_batch = &messages[0];
        let oldest_batch_date = oldest_batch.timestamp.with_timezone(&Local).date_naive();

        let old_bottom_idx = self.list.len().checked_sub(1);
        let old_bottom_row = old_bottom_idx.and_then(|i| self.get_row(i));

        let boundary_group = match &old_bottom_row {
            Some(ChatRow::Message { message: prev, .. }) => {
                self.last_message_date == Some(oldest_batch_date) && same_group(prev, oldest_batch)
            }
            _ => false,
        };

        let mut rows = Vec::with_capacity(messages.len());
        let mut metas = Vec::with_capacity(messages.len());
        let mut prev_msg: Option<&ChatMessage> = None;

        for (i, msg) in messages.iter().enumerate() {
            let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

            let mut first = true;
            if i == 0 {
                first = !boundary_group;
            } else if self.last_message_date == Some(msg_date)
                && let Some(prev) = prev_msg
            {
                first = !same_group(prev, msg);
            }

            // Insert a date separator if the date changed.
            if self.last_message_date != Some(msg_date) {
                rows.push(ChatRow::DateSeparator(msg_date));
                metas.push(RowMetadata::Separator(msg_date));
                self.last_message_date = Some(msg_date);
            }

            let last = messages.get(i + 1).is_none_or(|next| {
                let next_date = next.timestamp.with_timezone(&Local).date_naive();
                next_date != msg_date || !same_group(msg, next)
            });

            let ts = msg.timestamp.timestamp();
            rows.push(ChatRow::Message {
                last,
                first,
                message: msg.clone(),
            });
            metas.push(RowMetadata::Message(ts));
            prev_msg = Some(msg);
        }

        let objects: Vec<glib::BoxedAnyObject> =
            rows.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store().splice(self.list.len(), 0, &objects);

        self.row_metadata.extend(metas);

        if boundary_group
            && let (
                Some(idx),
                Some(ChatRow::Message {
                    last: _,
                    first,
                    message,
                }),
            ) = (old_bottom_idx, old_bottom_row)
        {
            let updated = ChatRow::Message {
                last: false,
                first,
                message,
            };
            let object = glib::BoxedAnyObject::new(updated);
            self.store().splice(idx, 1, &[object]);
        }

        // Update the newest loaded timestamp cursor.
        if let Some(newest) = messages.last() {
            self.newest_loaded_timestamp = Some(newest.timestamp.timestamp());
        }

        true
    }

    /// Trim rows from the bottom to stay within `max_rows`.
    pub(crate) fn trim_bottom(&mut self, max_rows: u32) -> u32 {
        let total = self.list.len();
        if total <= max_rows {
            return 0;
        }

        let to_remove = total - max_rows;
        let empty: Vec<glib::BoxedAnyObject> = Vec::new();
        self.store().splice(max_rows, to_remove, &empty);
        self.row_metadata.truncate(max_rows as usize);
        self.has_newer = true;

        // Update bottom cursors from remaining metadata.
        self.update_bottom_cursors();

        if let Some(idx) = self
            .row_metadata
            .iter()
            .rposition(|m| matches!(m, RowMetadata::Message(_)))
            .and_then(|i| u32::try_from(i).ok())
            && let Some(ChatRow::Message {
                last,
                first,
                message,
            }) = self.get_row(idx)
            && !last
        {
            let updated = ChatRow::Message {
                last: true,
                first,
                message,
            };
            let object = glib::BoxedAnyObject::new(updated);
            self.store().splice(idx, 1, &[object]);
        }

        to_remove
    }

    /// Trim rows from the top to stay within `max_rows`.
    pub(crate) fn trim_top(&mut self, max_rows: u32) -> u32 {
        let total = self.list.len();
        if total <= max_rows {
            return 0;
        }
        let to_remove = total - max_rows;

        if let Some(scrolled_window) = self
            .list
            .view
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
        {
            scrolled_window.set_kinetic_scrolling(false);
            scrolled_window.set_kinetic_scrolling(true);
        }

        let empty: Vec<glib::BoxedAnyObject> = Vec::new();
        self.store().splice(0, to_remove, &empty);
        self.row_metadata.drain(..to_remove as usize);
        self.has_older = true;

        // Update top cursors from remaining metadata.
        self.update_top_cursors();

        if let Some(idx) = self
            .row_metadata
            .iter()
            .position(|m| matches!(m, RowMetadata::Message(_)))
            .and_then(|i| u32::try_from(i).ok())
            && let Some(ChatRow::Message {
                last,
                first,
                message,
            }) = self.get_row(idx)
            && !first
        {
            let updated = ChatRow::Message {
                last,
                first: true,
                message,
            };
            let object = glib::BoxedAnyObject::new(updated);
            self.store().splice(idx, 1, &[object]);
        }

        to_remove
    }

    /// Find the index of the first message row matching `predicate`.
    pub(crate) fn find_message_index(
        &self,
        predicate: impl Fn(&ChatMessage) -> bool,
    ) -> Option<u32> {
        self.list
            .find(|row| matches!(row, ChatRow::Message { message, .. } if predicate(message)))
    }

    /// Get a cloned copy of the row at `index`.
    pub(crate) fn get_row(&self, index: u32) -> Option<ChatRow> {
        self.list.get(index).map(|item| item.borrow().clone())
    }

    /// Remove the row at `index` and re-insert a replacement, preserving scroll.
    pub(crate) fn replace_row(&self, index: u32, new_row: ChatRow, at_bottom: bool) {
        let adj = self.list.view.vadjustment();
        let saved_scroll = if at_bottom {
            None
        } else {
            adj.as_ref().map(AdjustmentExt::value)
        };

        let object = glib::BoxedAnyObject::new(new_row);
        self.store().splice(index, 1, &[object]);

        if let (Some(adj), Some(value)) = (adj, saved_scroll) {
            glib::idle_add_local_once(move || adj.set_value(value));
        }
    }

    /// Scroll the list view to the bottom (last row).
    pub(crate) fn scroll_to_bottom(&self) {
        let view = self.list.view.clone();
        glib::idle_add_local_once(move || {
            let count = view.model().map_or(0, |model| model.n_items());
            if count > 0 {
                let info = gtk::ScrollInfo::new();
                info.set_enable_vertical(true);
                view.scroll_to(count - 1, gtk::ListScrollFlags::FOCUS, Some(info));
            }
        });
    }

    /// The backing `gio::ListStore`, recovered through the public
    /// selection model so row batches can be spliced with a single
    /// items-changed emission.
    fn store(&self) -> gio::ListStore {
        self.list
            .selection_model
            .model()
            .and_then(|model| model.downcast::<gio::ListStore>().ok())
            .expect("unfiltered list view wraps the raw store")
    }

    /// Update bottom cursors (`newest_loaded_timestamp`, `last_message_date`)
    /// from the `row_metadata` after trimming rows from the bottom.
    fn update_bottom_cursors(&mut self) {
        self.last_message_date = None;
        self.newest_loaded_timestamp = None;

        // Walk backward through metadata to find the newest message and last date.
        for meta in self.row_metadata.iter().rev() {
            match meta {
                RowMetadata::Message(ts) => {
                    if self.newest_loaded_timestamp.is_none() {
                        self.newest_loaded_timestamp = Some(*ts);
                    }
                }
                RowMetadata::Separator(date) => {
                    if self.last_message_date.is_none() {
                        self.last_message_date = Some(*date);
                    }
                }
            }

            // Stop once both cursors are found.
            if self.newest_loaded_timestamp.is_some() && self.last_message_date.is_some() {
                break;
            }
        }
    }

    /// Update top cursors (`oldest_loaded_timestamp`, `first_message_date`)
    /// from the `row_metadata` after trimming rows from the top.
    fn update_top_cursors(&mut self) {
        self.first_message_date = None;
        self.oldest_loaded_timestamp = None;

        // Walk forward through metadata to find the oldest message and first date.
        for meta in &self.row_metadata {
            match meta {
                RowMetadata::Message(ts) => {
                    if self.oldest_loaded_timestamp.is_none() {
                        self.oldest_loaded_timestamp = Some(*ts);
                    }
                }
                RowMetadata::Separator(date) => {
                    if self.first_message_date.is_none() {
                        self.first_message_date = Some(*date);
                    }
                }
            }

            // Stop once both cursors are found.
            if self.oldest_loaded_timestamp.is_some() && self.first_message_date.is_some() {
                break;
            }
        }

        // We trimmed from top, so there are definitely older messages to load.
        self.has_older = true;
    }
}
