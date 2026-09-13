use toasty::{Deferred, Model};

use super::Message;

/// A chat conversation stored in the per-session database.
#[derive(Debug, Model)]
pub struct Chat {
    #[key]
    pub jid: String,
    pub name: String,
    pub muted: bool,
    pub pinned: bool,
    #[index]
    pub archived: bool,
    #[has_many]
    pub messages: Deferred<Vec<Message>>,
    pub last_message_time: Option<i64>,
}
