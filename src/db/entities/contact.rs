use toasty::Model;

#[derive(Debug, Model)]
pub struct Contact {
    #[key]
    pub jid: String,
    #[index]
    pub name: Option<String>,
    pub push_name: Option<String>,
    pub last_updated: i64,
    pub phone_number: Option<String>,
    pub is_registered: bool,
    pub profile_picture_url: Option<String>,
}
