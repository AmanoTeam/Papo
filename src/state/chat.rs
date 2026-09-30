use std::collections::HashMap;

use jiff::Timestamp;
use uuid::Uuid;

use crate::{db::store::SessionStore, state::ChatMessage, utils::format_lid_as_number};

#[derive(Clone, Debug)]
pub struct Chat {
    pub jid: String,
    pub name: String,
    pub muted: bool,
    pub pinned: bool,
    /// Whether this chat is archived.
    pub archived: bool,
    pub available: Option<bool>,
    pub last_seen: Option<Timestamp>,
    pub avatar_path: Option<String>,
    /// Participants names in groups (JID -> name).
    pub participants: HashMap<String, String>,
    pub last_message_time: Timestamp,
}

impl Chat {
    pub async fn upsert(&self, store: &SessionStore) -> Result<(), toasty::Error> {
        store.save_chat(self).await
    }

    pub fn is_group(&self) -> bool {
        self.jid.ends_with("@g.us")
    }

    pub async fn mark_read(
        &self,
        store: &SessionStore,
        own_chat: bool,
    ) -> Result<Vec<Uuid>, toasty::Error> {
        store.mark_chat_read(&self.jid, own_chat).await
    }

    /// Get the chat name or phone number if empty.
    pub fn get_name_or_number(&self) -> String {
        if self.name.is_empty() {
            format_lid_as_number(&self.jid)
        } else {
            self.name.clone()
        }
    }

    pub async fn get_last_message(
        &self,
        store: &SessionStore,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        store.load_messages(&self.jid, 1).await.map(|mut m| m.pop())
    }

    pub async fn load_messages(
        &self,
        store: &SessionStore,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        store.load_messages(&self.jid, limit).await
    }

    pub async fn load_messages_after(
        &self,
        store: &SessionStore,
        after_timestamp: Timestamp,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        store
            .load_messages_after(&self.jid, after_timestamp, limit)
            .await
    }

    pub async fn load_messages_before(
        &self,
        store: &SessionStore,
        before_timestamp: Timestamp,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        store
            .load_messages_before(&self.jid, before_timestamp, limit)
            .await
    }

    pub async fn find_message(
        &self,
        store: &SessionStore,
        msg_id: &str,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        store.load_message_by_server_id(&self.jid, msg_id).await
    }

    pub async fn find_message_by_local_id(
        &self,
        store: &SessionStore,
        msg_id: &Uuid,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        store.load_message_by_local_id(&self.jid, msg_id).await
    }

    pub async fn get_unread_count(&self, store: &SessionStore) -> Result<usize, toasty::Error> {
        store.get_unread_count(&self.jid).await
    }

    pub async fn get_unread_messages(
        &self,
        store: &SessionStore,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        store.get_unread_messages(&self.jid).await
    }
}

/// A user currently typing in a chat, and whether they are recording audio.
#[derive(Clone, Debug)]
pub struct TypingSender {
    pub name: String,
    pub recording: bool,
}
