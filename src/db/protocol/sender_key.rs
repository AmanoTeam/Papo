use toasty::Model;

#[derive(Debug, Model)]
pub struct SenderKey {
    pub record: Vec<u8>,
    #[key]
    pub address: String,
}
