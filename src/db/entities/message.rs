use jiff::Timestamp;
use toasty::{Deferred, Model};

use super::Chat;

/// A single chat message, including media file references.
#[derive(Debug, Model)]
#[index(chat_jid, timestamp)]
pub struct Message {
    #[belongs_to(key = chat_jid, references = jid)]
    pub chat: Deferred<Chat>,
    pub status: i64,
    pub content: Option<String>,
    pub chat_jid: String,
    #[key]
    pub local_id: String,
    pub outgoing: bool,
    #[unique]
    pub server_id: Option<String>,
    pub timestamp: Timestamp,
    pub sender_jid: String,
    pub sender_name: Option<String>,

    pub media_mime: Option<String>,
    pub media_path: Option<String>,
    pub media_type: Option<String>,

    pub quote_body: Option<String>,
    pub quote_media: Option<String>,
    pub quote_stanza_id: Option<String>,
    pub quote_participant: Option<String>,
    pub quote_sender_name: Option<String>,
}
