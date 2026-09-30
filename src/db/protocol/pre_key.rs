use toasty::Model;

#[derive(Debug, Model)]
#[table = "prekeys"]
pub struct PreKey {
    #[key]
    #[auto]
    pub id: i64,
    pub record: Vec<u8>,
    pub uploaded: bool,
}
