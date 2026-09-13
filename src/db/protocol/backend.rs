use whatsapp_rust::{serde_json, wacore::store::error::StoreError};

use crate::db::store::SessionStore;

#[derive(Clone)]
pub struct ProtocolBackend {
    pub(super) store: SessionStore,
}

impl ProtocolBackend {
    pub fn new(store: SessionStore) -> Self {
        Self { store }
    }
}

pub(super) fn db_err(error: toasty::Error) -> StoreError {
    StoreError::Database(Box::new(error))
}

pub(super) fn serde_err(error: serde_json::Error) -> StoreError {
    StoreError::Serialization(Box::new(error))
}
