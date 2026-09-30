use toasty::Model;

#[derive(Debug, Model)]
#[table = "device"]
pub struct DeviceState {
    #[key]
    pub id: i64, // Single-row table; id is pinned to 0, not auto-incremented.
    pub data: String,
}
