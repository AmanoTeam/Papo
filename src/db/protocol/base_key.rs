use toasty::Model;

#[derive(Debug, Model)]
#[key(address, message_id)]
pub struct BaseKey {
    pub key: Vec<u8>,
    pub address: String,
    pub message_id: String,
}
