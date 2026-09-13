use toasty::Model;

#[derive(Debug, Model)]
pub struct GroupMetadata {
    pub data: Vec<u8>,
    #[key]
    pub group_jid: String,
}
