use toasty::Model;

#[derive(Debug, Model)]
#[key(group_jid, device_jid)]
pub struct SenderKeyDevice {
    pub has_key: bool,
    pub group_jid: String,
    pub device_jid: String,
}
