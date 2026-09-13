use toasty::Model;

#[derive(Debug, Model)]
pub struct SyncKey {
    #[key]
    #[auto]
    pub id: i64,
    #[unique]
    pub key_id: String, // Stored as hex; the store layer encodes/decodes the raw key bytes.
    pub key_data: Vec<u8>,
    pub timestamp: i64,
    pub fingerprint: Vec<u8>,
}
