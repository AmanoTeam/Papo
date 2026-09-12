use toasty::{Deferred, Model};

use super::Chat;

#[derive(Debug, Model)]
pub struct Message {
    #[belongs_to(key = chat_jid, references = jid)]
    chat: Deferred<Chat>,
    status: i64,
    content: Option<String>,
    #[index]
    chat_jid: String,
    #[key]
    local_id: String,
    outgoing: bool,
    #[unique]
    server_id: Option<String>,
    #[index]
    timestamp: i64,
    media_mime: Option<String>,
    media_path: Option<String>,
    media_type: Option<String>,
    sender_jid: String,
    sender_name: Option<String>,
}
