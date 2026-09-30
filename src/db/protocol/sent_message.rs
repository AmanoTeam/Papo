use toasty::Model;

#[derive(Debug, Model)]
#[key(chat_jid, message_id)]
pub struct SentMessage {
    pub payload: Vec<u8>,
    pub chat_jid: String,
    pub created_at: i64,
    pub message_id: String,
}
