use toasty::{Deferred, Model};

use super::Message;

#[derive(Debug, Model)]
pub struct Chat {
    #[key]
    jid: String,
    name: String,
    muted: bool,
    pinned: bool,
    archived: bool,
    #[has_many]
    messages: Deferred<Vec<Message>>,
    last_message_time: Option<i64>,
}
