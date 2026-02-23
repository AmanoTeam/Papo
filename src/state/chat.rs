use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};

use crate::{state::ChatMessage, store::Database};

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
    /// Number of unread messages.
    pub unread_count: u32,
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
}
