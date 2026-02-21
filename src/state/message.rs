use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use crate::{
    state::{Chat, Media},
    store::Database,
};

/// Maximum number of unique emoji reactions per message to prevent spam.
const MAX_REACTIONS_PER_MESSAGE: usize = 50;

/// A chat message.
#[derive(Clone, Debug)]
pub struct Message {
    /// Unique message identifier.
    pub id: String,
    /// JID (Jabbed ID) - unique chat identifier.
    pub chat_jid: String,
    /// Sender identifier.
    pub sender_jid: String,
    /// Sender's display name (push name, for group chats).
    pub sender_name: Option<String>,

    /// Media attached to this message.
    pub media: Option<Media>,
    /// Whether the message hasn't been read.
    pub unread: bool,
    /// Message text.
    pub content: String,
    /// Whether the message was sent by the current user.
    pub outgoing: bool,
    /// Reactions on this message (emoji -> [sender JID]).
    pub reactions: IndexMap<String, Vec<String>>,
    /// When the message was sent/received.
    pub timestamp: DateTime<Utc>,

    pub db: Arc<Database>,
}

impl Message {
    /// Insert or update the current message in the database.
    pub async fn save(&self) -> Result<(), libsql::Error> {
        self.db.save_message(&self.chat_jid, &self).await
    }

    /// Load the chat this message is attached to.
    pub async fn load_chat(&self) -> Result<Chat, libsql::Error> {
        self.db
            .load_chat(&self.chat_jid)
            .await
            .map(|c| c.expect("Failed to get chat attached to message"))
    }
}
