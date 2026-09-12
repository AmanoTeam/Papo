use toasty::{Deferred, Model};

use super::Message;

#[derive(Debug, Model)]
pub struct Chat {
    #[key]
    pub jid: String,
    pub name: String,
    pub muted: bool,
    pub pinned: bool,
    pub archived: bool,
    #[has_many]
    pub messages: Deferred<Vec<Message>>,
    pub last_message_time: Option<i64>,
}
