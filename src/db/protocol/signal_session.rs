use toasty::Model;

#[derive(Debug, Model)]
pub struct SignalSession {
    pub data: Vec<u8>,
    #[key]
    pub address: String,
}
