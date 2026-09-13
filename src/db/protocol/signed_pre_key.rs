use toasty::Model;

#[derive(Debug, Model)]
#[table = "signed_prekeys"]
pub struct SignedPreKey {
    #[key]
    #[auto]
    pub id: i64,
    pub record: Vec<u8>,
}
