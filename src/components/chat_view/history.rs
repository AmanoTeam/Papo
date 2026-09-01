use std::collections::VecDeque;

use adw::prelude::*;
use chrono::{Local, NaiveDate};
use gtk::glib;
use relm4::{prelude::*, typed_view::list::TypedListView};

use super::rows::ChatRow;
use crate::state::ChatMessage;

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

        for msg in messages.iter().rev() {
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

            let ts = msg.timestamp.timestamp();
            self.list.append(ChatRow::Message(msg.clone()));
            self.row_metadata.push_back(RowMetadata::Message(ts));
        }

        true
    }

    /// Append a single live message to the bottom. Updates only the newest
    /// cursor.
    pub(crate) fn append_live(&mut self, message: ChatMessage) {
        // Convert to local date for separator comparison.
        let msg_date = message.timestamp.with_timezone(&Local).date_naive();

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

        self.list.append(ChatRow::Message(message));
        self.row_metadata.push_back(RowMetadata::Message(ts));
    }

    /// Prepend a batch of older messages to the top; reversed internally.
    pub(crate) fn prepend_messages(&mut self, messages: &[ChatMessage]) -> u32 {
        if messages.is_empty() {
            return 0;
        }

        // Update the oldest loaded timestamp cursor.
        if let Some(oldest) = messages.last() {
            self.oldest_loaded_timestamp = Some(oldest.timestamp.timestamp());
        }

        // Reverse messages to get chronological order for prepending.
        let mut insert_pos: u32 = 0;
        let mut prev_date: Option<NaiveDate> = None;

        for msg in messages.iter().rev() {
            let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

            // Insert a date separator if the date changed.
            if prev_date != Some(msg_date) {
                self.list
                    .insert(insert_pos, ChatRow::DateSeparator(msg_date));
                self.row_metadata
                    .insert(insert_pos as usize, RowMetadata::Separator(msg_date));
                insert_pos += 1;
                prev_date = Some(msg_date);
            }

            let ts = msg.timestamp.timestamp();
            self.list.insert(insert_pos, ChatRow::Message(msg.clone()));
            self.row_metadata
                .insert(insert_pos as usize, RowMetadata::Message(ts));
            insert_pos += 1;
        }

        // Remove duplicate date separator if the last prepended date matches
        // the first existing date separator.
        if let Some(last_prepended_date) = prev_date
            && Some(last_prepended_date) == self.first_message_date
            && insert_pos < self.list.len()
        {
            self.list.remove(insert_pos);
            self.row_metadata.remove(insert_pos as usize);
        }

        // Update first_message_date to the oldest prepended message's date.
        if let Some(oldest_msg) = messages.last() {
            self.first_message_date = Some(oldest_msg.timestamp.with_timezone(&Local).date_naive());
        }

        insert_pos
    }

    /// Append a batch of newer messages to the bottom; iterated forward.
    pub(crate) fn append_newer(&mut self, messages: &[ChatMessage]) -> bool {
        if messages.is_empty() {
            return false;
        }

        for msg in messages {
            let msg_date = msg.timestamp.with_timezone(&Local).date_naive();

            // Insert a date separator if the date changed.
            if self.last_message_date != Some(msg_date) {
                self.list.append(ChatRow::DateSeparator(msg_date));
                self.row_metadata
                    .push_back(RowMetadata::Separator(msg_date));
                self.last_message_date = Some(msg_date);
            }

            let ts = msg.timestamp.timestamp();
            self.list.append(ChatRow::Message(msg.clone()));
            self.row_metadata.push_back(RowMetadata::Message(ts));
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
        for _ in 0..to_remove {
            self.list.remove(self.list.len() - 1);
            self.row_metadata.pop_back();
        }

        self.has_newer = true;

        // Update bottom cursors from remaining metadata.
        self.update_bottom_cursors();
        to_remove
    }

    /// Trim rows from the top to stay within `max_rows`.
    pub(crate) fn trim_top(&mut self, max_rows: u32) -> u32 {
        let total = self.list.len();
        if total <= max_rows {
            return 0;
        }

        let to_remove = total - max_rows;
        for _ in 0..to_remove {
            self.list.remove(0);
            self.row_metadata.pop_front();
        }

        self.has_older = true;

        // Update top cursors from remaining metadata.
        self.update_top_cursors();
        to_remove
    }

    /// Find the index of the first message row matching `predicate`.
    pub(crate) fn find_message_index(
        &self,
        predicate: impl Fn(&ChatMessage) -> bool,
    ) -> Option<u32> {
        self.list
            .find(|row| matches!(row, ChatRow::Message(msg) if predicate(msg)))
    }

    /// Get a cloned copy of the row at `index`.
    pub(crate) fn get_row(&self, index: u32) -> Option<ChatRow> {
        self.list.get(index).map(|item| item.borrow().clone())
    }

    /// Remove the row at `index` and re-insert a replacement, preserving scroll.
    pub(crate) fn replace_row(&mut self, index: u32, new_row: ChatRow) {
        let adj = self.list.view.vadjustment();
        let saved_scroll = adj.as_ref().map(AdjustmentExt::value);

        self.list.remove(index);
        self.list.insert(index, new_row);

        if let (Some(adj), Some(value)) = (adj, saved_scroll) {
            glib::idle_add_local_once(move || adj.set_value(value));
        }
    }

    /// Scroll the list view to the bottom (last row).
    pub(crate) fn scroll_to_bottom(&self) {
        let count = self.list.len();
        if count > 0 {
            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            self.list
                .view
                .scroll_to(count - 1, gtk::ListScrollFlags::FOCUS, Some(info));
        }
    }

    /// Re-anchor the scroll to the row at `index` once the list
    /// view has reallocated row heights, then run `settled`.
    pub(crate) fn anchor_scroll(&self, index: u32, settled: impl FnOnce() + 'static) {
        if index == 0 || index >= self.list.len() {
            settled();
            return;
        }

        let view = self.list.view.clone();
        glib::idle_add_local_once(move || {
            let info = gtk::ScrollInfo::new();
            info.set_enable_vertical(true);
            view.scroll_to(index, gtk::ListScrollFlags::NONE, Some(info));

            settled();
        });
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
