use toasty::{Deferred, Model};

use super::Chat;

#[derive(Debug, Model)]
pub struct Message {
    #[belongs_to(key = chat_jid, references = jid)]
    pub chat: Deferred<Chat>,
    pub status: i64,
    pub content: Option<String>,
    #[index]
    pub chat_jid: String,
    #[key]
    pub local_id: String,
    pub outgoing: bool,
    #[unique]
    pub server_id: Option<String>,
    #[index]
    pub timestamp: i64,
    pub media_mime: Option<String>,
    pub media_path: Option<String>,
    pub media_type: Option<String>,
    pub sender_jid: String,
    pub sender_name: Option<String>,
}
