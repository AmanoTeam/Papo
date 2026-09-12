use toasty::Model;

#[derive(Clone, Debug, Model)]
pub struct Session {
    pub name: Option<String>,
    pub path: String,
    #[key]
    pub uuid: String,
    pub phone: Option<String>,
    pub created_at: i64,
    pub last_active: i64,
}
