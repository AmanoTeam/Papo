use toasty::Model;

#[derive(Debug, Model)]
pub struct TcToken {
    #[key]
    pub jid: String,
    pub token: Vec<u8>,
    pub token_timestamp: i64,
    pub sender_timestamp: Option<i64>,
}
