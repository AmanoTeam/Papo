use toasty::Model;

/// A chat read mark queued for the multi-device appstate sync.
#[derive(Debug, Model)]
pub struct PendingReadMark {
    #[key]
    pub chat_jid: String,
    /// Read position pinned when the mark was made, in unix seconds.
    /// A retry must not mark messages read that arrived since.
    pub position: i64,
    /// Consecutive failed sync attempts; drives the retry backoff.
    pub attempts: i64,
    /// Unix timestamp of the earliest allowed next attempt.
    pub next_retry_at: i64,
}
