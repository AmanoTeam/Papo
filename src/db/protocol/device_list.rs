use toasty::Model;

#[derive(Debug, Model)]
pub struct DeviceList {
    #[key]
    pub user: String,
    pub record: String,
}
