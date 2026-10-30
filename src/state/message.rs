use indexmap::IndexMap;
use jiff::Timestamp;
use uuid::Uuid;
use whatsapp_rust::{types::presence::ReceiptType, waproto::whatsapp as wa};

use crate::{
    db::store::SessionStore,
    i18n,
    state::Media,
    utils::{bare_jid, is_group_jid},
};

/// Maximum number of unique emoji reactions per message to prevent spam.
const MAX_REACTIONS_PER_MESSAGE: usize = 50;

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub media: Option<Media>,
    /// The reply context, if this message quotes another.
    pub quote: Option<ChatQuote>,
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
    /// Whether `sender_name` came from a push source rather than a
    /// saved contact.
    pub sender_push: bool,
}

impl ChatMessage {
    /// The sender's name as it should paint: contact-resolved names
    /// plain, push-sourced names carrying the tilde marker.
    pub fn display_sender_name(&self) -> Option<String> {
        self.sender_name.as_deref().map(|name| {
            if self.sender_push {
                format!("~{name}")
            } else {
                name.to_owned()
            }
        })
    }

    pub async fn upsert(&self, store: &SessionStore) -> Result<(), toasty::Error> {
        store.save_message(&self.chat_jid, self).await
    }

    /// Insert the message, skipping if a duplicate `server_id` already exists.
    /// Also ensures the chat exists for foreign key satisfaction.
    /// Returns `true` if inserted, `false` if skipped as duplicate.
    pub async fn save_or_ignore(&self, store: &SessionStore) -> Result<bool, toasty::Error> {
        store.save_synced_message(&self.chat_jid, self).await
    }

    /// Decodes the reply context from the wire: `contextInfo` appears
    /// inside per-type messages rather than the shared envelope, so
    /// every carrier we render replies on is checked.
    pub(crate) fn wire_quote(message: &wa::Message) -> Option<ChatQuote> {
        let context_info = message
            .extended_text_message
            .as_option()
            .and_then(|ext| ext.context_info.as_option())
            .or_else(|| {
                message
                    .image_message
                    .as_option()
                    .and_then(|img| img.context_info.as_option())
            })
            .or_else(|| {
                message
                    .video_message
                    .as_option()
                    .and_then(|video| video.context_info.as_option())
            })
            .or_else(|| {
                message
                    .audio_message
                    .as_option()
                    .and_then(|audio| audio.context_info.as_option())
            })
            .or_else(|| {
                message
                    .document_message
                    .as_option()
                    .and_then(|doc| doc.context_info.as_option())
            })
            .or_else(|| {
                message
                    .sticker_message
                    .as_option()
                    .and_then(|sticker| sticker.context_info.as_option())
            })?;
        let quoted = context_info.quoted_message.as_option()?;

        let body = quoted
            .conversation
            .clone()
            .or_else(|| {
                quoted
                    .extended_text_message
                    .as_option()
                    .and_then(|ext| ext.text.clone())
            })
            .filter(|text| !text.is_empty());
        let media = (body.is_none()).then(|| {
            if quoted.image_message.is_set() {
                i18n!("Photo")
            } else if quoted.video_message.is_set() {
                i18n!("Video")
            } else if quoted.audio_message.is_set() {
                i18n!("Voice message")
            } else if quoted.document_message.is_set() {
                i18n!("Document")
            } else if quoted.sticker_message.is_set() {
                i18n!("Sticker")
            } else {
                i18n!("Message")
            }
        });
        let participant = context_info
            .participant
            .as_deref()
            .map(bare_jid)
            .filter(|participant| !participant.is_empty() && !is_group_jid(participant));

        Some(ChatQuote {
            body,
            media,
            stanza_id: context_info.stanza_id.clone(),
            participant,
            sender_name: None,
            sender_push: false,
        })
    }
}

impl From<ChatMessage> for wa::Message {
    fn from(value: ChatMessage) -> Self {
        let content = if value.content.is_empty() {
            None
        } else {
            Some(value.content)
        };

        if let Some(quote) = value.quote {
            return Self {
                extended_text_message: wa::message::ExtendedTextMessage {
                    text: content,
                    context_info: wa::ContextInfo {
                        stanza_id: quote.stanza_id,
                        participant: quote.participant,
                        quoted_message: Self {
                            conversation: quote.body.filter(|body| !body.is_empty()),
                            ..Default::default()
                        }
                        .into(),
                        ..Default::default()
                    }
                    .into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            };
        }

        Self {
            conversation: content,
            ..Default::default()
        }
    }
}

impl From<&ChatMessage> for wa::Message {
    fn from(value: &ChatMessage) -> Self {
        value.to_owned().into()
    }
}

/// The quoted context of a reply, decoded from the wire. The stanza id
/// and author survive forever.
#[derive(Clone, Debug)]
pub struct ChatQuote {
    pub body: Option<String>,
    pub media: Option<String>,
    pub stanza_id: Option<String>,
    pub participant: Option<String>,
    pub sender_name: Option<String>,
    /// Whether `sender_name` is push-sourced.
    pub sender_push: bool,
}

impl ChatQuote {
    /// The quoted author's name as it should paint.
    pub fn display_sender_name(&self) -> Option<String> {
        self.sender_name.as_deref().map(|name| {
            if self.sender_push {
                format!("~{name}")
            } else {
                name.to_owned()
            }
        })
    }

    /// Display text for the snippet: the quoted body, else the media
    /// type label.
    pub fn text(&self) -> Option<&str> {
        self.body
            .as_deref()
            .filter(|body| !body.is_empty())
            .or(self.media.as_deref())
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
