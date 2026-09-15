use indexmap::IndexMap;
use jiff::Timestamp;
use uuid::Uuid;
use whatsapp_rust::{types::presence::ReceiptType, waproto::whatsapp as wa};

use crate::{
    db::store::SessionStore,
    state::{Chat, Media},
};

/// Maximum number of unique emoji reactions per message to prevent spam.
const MAX_REACTIONS_PER_MESSAGE: usize = 50;

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub db: SessionStore,
    pub media: Option<Media>,
    pub status: MessageStatus,
    pub content: String,
    pub chat_jid: String,
    pub local_id: Uuid,
    pub outgoing: bool,
    /// Reactions on this message (emoji -> [sender JID]).
    pub reactions: IndexMap<String, Vec<String>>,
    pub sender_jid: String,
    pub server_id: String,
    pub timestamp: Timestamp,
    /// Sender's display name (push name, for group chats).
    pub sender_name: Option<String>,
}

impl ChatMessage {
    pub async fn upsert(&self) -> Result<(), toasty::Error> {
        self.db.save_message(&self.chat_jid, self).await
    }

    /// Insert the message, skipping if a duplicate `server_id` already exists.
    /// Also ensures the chat exists for foreign key satisfaction.
    /// Returns `true` if inserted, `false` if skipped as duplicate.
    pub async fn save_or_ignore(&self) -> Result<bool, toasty::Error> {
        self.db.save_synced_message(&self.chat_jid, self).await
    }

    pub async fn load_chat(&self) -> Result<Chat, toasty::Error> {
        self.db
            .load_chat(&self.chat_jid)
            .await
            .map(|c| c.expect("Failed to get chat attached to message"))
    }

    pub async fn mark_read(&mut self) -> Result<(), toasty::Error> {
        if matches!(self.status, MessageStatus::Read | MessageStatus::Played) {
            Ok(())
        } else {
            self.status = MessageStatus::Read;
            self.upsert().await
        }
    }
}

impl From<ChatMessage> for wa::Message {
    fn from(value: ChatMessage) -> Self {
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

impl From<&ChatMessage> for wa::Message {
    fn from(value: &ChatMessage) -> Self {
        value.to_owned().into()
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(u8)]
pub enum MessageStatus {
    /// The message was sent, but no one has received it yet.
    #[default]
    Sent,
    /// The message was read by all.
    Read,
    Failed,
    /// The message's media has been played.
    Played,
    Sending,
    /// The recipient(s) has received the message.
    Delivered,
}

impl MessageStatus {
    /// Position along the delivery lifecycle: `sending < sent < delivered <
    /// read < played`. Receipts may only advance the status forward. `Failed`
    /// is not a lifecycle stage; it is handled separately.
    pub fn stage(self) -> u8 {
        match self {
            Self::Sending | Self::Failed => 0,
            Self::Sent => 1,
            Self::Delivered => 2,
            Self::Read => 3,
            Self::Played => 4,
        }
    }

    pub fn icon_name(&self) -> &str {
        match self {
            Self::Sent => "check-plain-symbolic",
            Self::Delivered => "check-round-outline-symbolic",
            Self::Read | Self::Played => "check-round-outline2-symbolic",
            Self::Failed => "exclamation-mark-symbolic",
            Self::Sending => "clock-alt-symbolic",
        }
    }
}

impl From<i32> for MessageStatus {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Sent,
            1 => Self::Read,
            2 => Self::Failed,
            3 => Self::Played,
            4 => Self::Sending,
            5 => Self::Delivered,
            _ => Self::default(),
        }
    }
}

impl TryFrom<ReceiptType> for MessageStatus {
    type Error = String;

    fn try_from(value: ReceiptType) -> Result<Self, Self::Error> {
        match value {
            ReceiptType::Read | ReceiptType::ReadSelf => Ok(Self::Read),
            ReceiptType::Retry | ReceiptType::ServerError => Ok(Self::Failed),
            ReceiptType::Played | ReceiptType::PlayedSelf => Ok(Self::Played),
            ReceiptType::Sender => Ok(Self::Sent),
            ReceiptType::Delivered => Ok(Self::Delivered),
            ReceiptType::Other(t) if t == "delivery" => Ok(Self::Delivered),
            r => Err(format!("Message status doesn't have a {r:?} equivalent")),
        }
    }
}
