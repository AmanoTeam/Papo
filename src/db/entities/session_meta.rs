use toasty::Model;

#[derive(Debug, Model)]
pub struct Session {
    name: Option<String>,
    path: String,
    #[key]
    uuid: String,
    phone: Option<String>,
    created_at: i64,
    last_active: i64,
}
