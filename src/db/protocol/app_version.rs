use toasty::Model;

#[derive(Debug, Model)]
pub struct AppVersion {
    pub hash: Option<Vec<u8>>,
    #[key]
    pub name: String,
    pub version: i64,
    pub index_value_map: Option<String>,
}
