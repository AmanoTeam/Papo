use toasty::Model;

#[derive(Debug, Model)]
pub struct Contact {
    #[key]
    jid: String,
    #[index]
    name: Option<String>,
    push_name: Option<String>,
    last_updated: i64,
    phone_number: Option<String>,
    is_registered: bool,
    profile_picture_url: Option<String>,
}
