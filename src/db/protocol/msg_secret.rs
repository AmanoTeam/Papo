use toasty::Model;

#[derive(Debug, Model)]
#[key(chat, sender, msg_id)]
pub struct MsgSecret {
    pub chat: String,
    pub msg_id: String,
    pub secret: Vec<u8>,
    pub sender: String,
    pub expires_at: i64,
    pub message_ts: i64,
}
