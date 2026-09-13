use toasty::Model;

#[derive(Debug, Model)]
#[table = "pending_inbound"]
#[key(chat, sender, id)]
pub struct PendingInbound {
    pub id: String,
    pub chat: String,
    pub sender: String,
    pub message: Vec<u8>,
}
