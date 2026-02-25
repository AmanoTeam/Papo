use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};

use crate::{state::ChatMessage, store::Database, utils::format_lid_as_number};

/// A chat/conversation.
#[derive(Clone, Debug)]
pub struct Chat {
    /// JID (Jabbed ID) - unique chat identifier.
    pub jid: String,
    /// Display name.
    pub name: String,
    /// Whether the chat is muted.
    pub muted: bool,
    /// Whether this chat is pinned.
    pub pinned: bool,
    /// Participants names in groups (JID -> name).
    pub participants: HashMap<String, String>,
    /// Time of the last sent message.
    pub last_message_time: DateTime<Utc>,

    pub db: Arc<Database>,
}

impl Chat {
    /// Insert or update the current chat in the database.
    pub async fn save(&self) -> Result<(), libsql::Error> {
        self.db.save_chat(self).await
    }

    /// Check if the chat is a group.
    pub fn is_group(&self) -> bool {
        self.jid.ends_with("@g.us")
    }

    /// Mark all messages in this chat as read.
    pub async fn mark_read(&self) -> Result<(), libsql::Error> {
        if self.get_unread_count().await.is_ok_and(|count| count > 0) {
            self.db
                .execute(
                    "UPDATE messages SET unread = 0 WHERE chat_jid = ?1",
                    [self.jid.as_str()],
                )
                .await
                .map(drop)
        } else {
            Ok(())
        }
    }

    /// Get the chat name or phone number if empty.
    pub fn get_name_or_number(&self) -> String {
        if self.name.is_empty() {
            format_lid_as_number(&self.jid)
        } else {
            self.name.clone()
        }
    }

    /// Get the last sent message in this chat.
    pub async fn get_last_message(&self) -> Result<Option<ChatMessage>, libsql::Error> {
        self.load_messages(1).await.map(|mut m| m.pop())
    }

    /// Load a specified amount of messages in this chat.
    pub async fn load_messages(&self, limit: u32) -> Result<Vec<ChatMessage>, libsql::Error> {
        self.db.load_messages(&self.jid, limit).await
    }

    /// Find a message in this chat by its ID.
    pub async fn find_message(&self, msg_id: &str) -> Result<Option<ChatMessage>, libsql::Error> {
        self.db.load_message(&self.jid, msg_id).await
    }

    /// Get the count of unread messages in this chat.
    pub async fn get_unread_count(&self) -> Result<usize, libsql::Error> {
        self.db.get_unread_count(&self.jid).await
    }

    /// Get all unread messages in this chat.
    pub async fn get_unread_messages(&self) -> Result<Vec<ChatMessage>, libsql::Error> {
        self.db.get_unread_messages(&self.jid).await
    }
}
