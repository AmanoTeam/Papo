use std::collections::HashMap;

use jiff::Timestamp;
use uuid::Uuid;

use crate::{db::store::SessionStore, state::ChatMessage, utils::format_lid_as_number};

#[derive(Clone, Debug)]
pub struct Chat {
    pub db: SessionStore,
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
    pub async fn upsert(&self) -> Result<(), toasty::Error> {
        self.db.save_chat(self).await
    }

    pub fn is_group(&self) -> bool {
        self.jid.ends_with("@g.us")
    }

    pub async fn mark_read(&self, own_chat: bool) -> Result<Vec<Uuid>, toasty::Error> {
        self.db.mark_chat_read(&self.jid, own_chat).await
    }

    /// Get the chat name or phone number if empty.
    pub fn get_name_or_number(&self) -> String {
        if self.name.is_empty() {
            format_lid_as_number(&self.jid)
        } else {
            self.name.clone()
        }
    }

    pub async fn get_last_message(&self) -> Result<Option<ChatMessage>, toasty::Error> {
        self.load_messages(1).await.map(|mut m| m.pop())
    }

    pub async fn load_messages(&self, limit: u32) -> Result<Vec<ChatMessage>, toasty::Error> {
        self.db.load_messages(&self.jid, limit as usize).await
    }

    pub async fn load_messages_after(
        &self,
        after_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        self.db
            .load_messages_after(&self.jid, after_timestamp, limit as usize)
            .await
    }

    pub async fn load_messages_before(
        &self,
        before_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        self.db
            .load_messages_before(&self.jid, before_timestamp, limit as usize)
            .await
    }

    pub async fn find_message(&self, msg_id: &str) -> Result<Option<ChatMessage>, toasty::Error> {
        self.db.load_message_by_server_id(&self.jid, msg_id).await
    }

    pub async fn find_message_by_local_id(
        &self,
        msg_id: &Uuid,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        self.db.load_message_by_local_id(&self.jid, msg_id).await
    }

    pub async fn get_unread_count(&self) -> Result<usize, toasty::Error> {
        self.db.get_unread_count(&self.jid).await
    }

    pub async fn get_unread_messages(&self) -> Result<Vec<ChatMessage>, toasty::Error> {
        self.db.get_unread_messages(&self.jid).await
    }
}

/// A user currently typing in a chat, and whether they are recording audio.
#[derive(Clone, Debug)]
pub struct TypingSender {
    pub name: String,
    pub recording: bool,
}
