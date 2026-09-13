use toasty::Model;

#[derive(Debug, Model)]
pub struct Identity {
    pub key: Vec<u8>,
    #[key]
    pub address: String,
}
