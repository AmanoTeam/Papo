use std::sync::Arc;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use uuid::Uuid;
use wacore::types::presence::ReceiptType;
use waproto::whatsapp as wa;

use crate::{
    i18n,
    state::{Chat, Media, MediaType},
    store::Database,
};

/// Maximum number of unique emoji reactions per message to prevent spam.
const MAX_REACTIONS_PER_MESSAGE: usize = 50;

/// Represents a chat message.
#[derive(Clone, Debug)]
pub struct Message {
    pub db: Arc<Database>,
    /// Media attached to this message.
    pub media: Option<Media>,
    /// Actual state of the message.
    pub status: Status,
    /// Message text.
    pub content: String,
    /// JID (Jabbed ID) - unique chat identifier.
    pub chat_jid: String,
    /// Local unique message identifier.
    pub local_id: Uuid,
    /// Whether the message was sent by the current user.
    pub outgoing: bool,
    /// Reactions on this message (emoji -> [sender JID]).
    pub reactions: IndexMap<String, Vec<String>>,
    /// Sender identifier.
    pub sender_jid: String,
    /// Server unique message identifier.
    pub server_id: String,
    /// When the message was sent/received.
    pub timestamp: DateTime<Utc>,
    /// Sender's display name (push name, for group chats).
    pub sender_name: Option<String>,
}

impl Message {
    /// Insert or update the current message in the database.
    pub async fn save(&self) -> Result<(), libsql::Error> {
        self.db.save_message(&self.chat_jid, self).await
    }

    /// Insert the message, skipping if a duplicate `server_id` already exists.
    /// Also ensures the chat exists for foreign key satisfaction.
    /// Returns `true` if inserted, `false` if skipped as duplicate.
    pub async fn save_or_ignore(&self) -> Result<bool, libsql::Error> {
        self.db.save_synced_message(&self.chat_jid, self).await
    }

    /// Load the chat this message is attached to.
    pub async fn load_chat(&self) -> Result<Chat, libsql::Error> {
        self.db
            .load_chat(&self.chat_jid)
            .await
            .map(|c| c.expect("Failed to get chat attached to message"))
    }

    /// Mark this message as read locally.
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
    /// The message's media has been played.
    Played,
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
            Self::Read | Self::Played | Self::Delivered => "check-round-outline2-symbolic",
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
            3 => Self::Played,
            4 => Self::Sending,
            5 => Self::Delivered,
            _ => Self::default(),
        }
    }
}

impl TryFrom<ReceiptType> for Status {
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

/// Returns a fallback display label for a `WhatsApp` message that has no
/// renderable text content.
///
/// Media types (image, video, audio, sticker, document) get a descriptive
/// label via [`MediaType::display_label`]. Other user-facing message types
/// (contacts, location, polls, calls, etc.) get a generic "Unsupported
/// message" string. Internal/protocol messages and wrapper types that are
/// handled separately by the caller return `None` so they can be silently
/// skipped.
pub fn unsupported_label(msg: &wa::Message) -> Option<String> {
    if msg.image_message.is_some() {
        return Some(MediaType::Image.display_label());
    }
    if msg.video_message.is_some() || msg.ptv_message.is_some() {
        return Some(MediaType::Video.display_label());
    }
    if msg.audio_message.is_some() {
        return Some(MediaType::Audio.display_label());
    }
    if msg.sticker_message.is_some() {
        return Some(MediaType::Sticker.display_label());
    }
    if msg.document_message.is_some() || msg.document_with_caption_message.is_some() {
        return Some(MediaType::Document.display_label());
    }

    if msg.sender_key_distribution_message.is_some()
        || msg
            .fast_ratchet_key_sender_key_distribution_message
            .is_some()
        || msg.protocol_message.is_some()
        || msg.sticker_sync_rmr_message.is_some()
        || msg.keep_in_chat_message.is_some()
        || msg.pin_in_chat_message.is_some()
        || msg.message_history_bundle.is_some()
        || msg.enc_reaction_message.is_some()
        || msg.reaction_message.is_some()
        || msg.poll_update_message.is_some()
        || msg.device_sent_message.is_some()
    {
        return None;
    }

    Some(i18n!("Unsupported message"))
}
