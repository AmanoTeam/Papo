use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use uuid::Uuid;
use waproto::whatsapp as wa;

use crate::{
    state::{Chat, Media},
    store::Database,
};

/// Maximum number of unique emoji reactions per message to prevent spam.
const MAX_REACTIONS_PER_MESSAGE: usize = 50;

/// Represents a chat message.
#[derive(Clone, Debug)]
pub struct Message {
    /// Local unique message identifier.
    pub local_id: Uuid,
    /// Server unique message identifier.
    pub server_id: String,
    /// JID (Jabbed ID) - unique chat identifier.
    pub chat_jid: String,
    /// Sender identifier.
    pub sender_jid: String,
    /// Sender's display name (push name, for group chats).
    pub sender_name: Option<String>,

    /// Media attached to this message.
    pub media: Option<Media>,
    /// Actual state of the message.
    pub status: Status,
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
        self.db.save_message(&self.chat_jid, self).await
    }

    /// Load the chat this message is attached to.
    pub async fn load_chat(&self) -> Result<Chat, libsql::Error> {
        self.db
            .load_chat(&self.chat_jid)
            .await
            .map(|c| c.expect("Failed to get chat attached to message"))
    }

    /// Mark this message as read.
    pub async fn mark_read(&mut self) -> Result<(), libsql::Error> {
        if self.status == Status::Read {
            Ok(())
        } else {
            self.status = Status::Read;
            self.save().await
        }
    }
}

impl From<Message> for wa::Message {
    fn from(value: Message) -> Self {
        let conversation = if value.content.is_empty() {
            None
        } else {
            Some(value.content)
        };

        Self {
            conversation,
            ..Default::default()
        }
    }
}

impl From<&Message> for wa::Message {
    fn from(value: &Message) -> Self {
        value.to_owned().into()
    }
}

/// Represents a message status.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum Status {
    /// The message was sent, but no one has received it yet.
    #[default]
    Sent,
    /// The message was read by all.
    Read,
    /// The message has failed to send.
    Failed,
    /// The message is being sent.
    Sending,
    /// The recipient(s) has received the message.
    Delivered,
}

impl Status {
    /// Get the corresponding status icon name.
    pub fn icon_name(&self) -> &str {
        match self {
            Self::Sent => "check-round-outline-symbolic",
            Self::Read | Self::Delivered => "check-round-outline2-symbolic",
            Self::Failed => "exclamation-mark-symbolic",
            Self::Sending => "clock-alt-symbolic",
        }
    }
}

impl From<i32> for Status {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Sent,
            1 => Self::Read,
            2 => Self::Failed,
            3 => Self::Sending,
            4 => Self::Delivered,
            _ => Self::default(),
        }
    }
}
