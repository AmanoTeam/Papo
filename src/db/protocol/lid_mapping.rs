use toasty::Model;

#[derive(Debug, Model)]
pub struct LidMapping {
    #[key]
    pub lid: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub phone_number: String,
    pub learning_source: String,
}
