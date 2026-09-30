use whatsapp_rust::{
    async_trait, serde_json,
    wacore::{
        appstate::{hash::HashState, processor::AppStateMutationMAC},
        store::{
            error::{Result as StoreResult, StoreError},
            traits::{AppStateSyncKey, AppSyncStore},
        },
    },
};

use super::{
    AppVersion, MutationMac, SyncKey,
    backend::{ProtocolBackend, db_err, serde_err},
};

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    out
}

fn hex_decode(value: &str) -> StoreResult<Vec<u8>> {
    fn nibble(byte: u8) -> StoreResult<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => {
                let digit = char::from(byte);
                Err(StoreError::Validation(format!(
                    "invalid hex digit: {digit}"
                )))
            }
        }
    }

    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(StoreError::Validation("odd-length hex string".into()));
    }

    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = nibble(pair[0])?;
        let low = nibble(pair[1])?;
        out.push((high << 4) | low);
    }

    Ok(out)
}

#[async_trait]
impl AppSyncStore for ProtocolBackend {
    async fn get_sync_key(&self, key_id: &[u8]) -> StoreResult<Option<AppStateSyncKey>> {
        let mut db = self.store.db().clone();
        Ok(SyncKey::filter_by_key_id(hex_encode(key_id))
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|entity| AppStateSyncKey {
                key_data: entity.key_data,
                fingerprint: entity.fingerprint,
                timestamp: entity.timestamp,
            }))
    }

    async fn set_sync_key(&self, key_id: &[u8], key: AppStateSyncKey) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        SyncKey::upsert_by_key_id(hex_encode(key_id))
            .key_data(key.key_data)
            .fingerprint(key.fingerprint)
            .timestamp(key.timestamp)
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn get_version(&self, name: &str) -> StoreResult<HashState> {
        let mut db = self.store.db().clone();
        let version = AppVersion::filter_by_name(name)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(version.map_or_else(HashState::default, |entity| HashState {
            version: u64::try_from(entity.version).unwrap_or(0),
            hash: entity
                .hash
                .and_then(|hash| hash.try_into().ok())
                .unwrap_or([0; 128]),
            index_value_map: entity
                .index_value_map
                .and_then(|map| serde_json::from_str(&map).ok())
                .unwrap_or_default(),
            // mac_mismatch_fatal has no entity column; a restart resets it and
            // the next snapshot-MAC failure re-arms it.
            mac_mismatch_fatal: false,
        }))
    }

    async fn set_version(&self, name: &str, state: HashState) -> StoreResult<()> {
        let index_value_map = serde_json::to_string(&state.index_value_map).map_err(serde_err)?;
        let version = i64::try_from(state.version)
            .map_err(|_| StoreError::Validation("app state version out of range".into()))?;
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        AppVersion::upsert_by_name(name)
            .hash(Some(state.hash.to_vec()))
            .version(version)
            .index_value_map(Some(index_value_map))
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn put_mutation_macs(
        &self,
        name: &str,
        _version: u64,
        mutations: &[AppStateMutationMAC],
    ) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        for mutation in mutations {
            MutationMac::upsert_by_name_and_index_mac(name, hex_encode(&mutation.index_mac))
                .value_mac(mutation.value_mac.clone())
                .exec(&mut db)
                .await
                .map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_mutation_mac(&self, name: &str, index_mac: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        let mut db = self.store.db().clone();
        Ok(
            MutationMac::filter_by_name_and_index_mac(name, hex_encode(index_mac))
                .first()
                .exec(&mut db)
                .await
                .map_err(db_err)?
                .map(|entity| entity.value_mac),
        )
    }

    async fn delete_mutation_macs(&self, name: &str, index_macs: &[Vec<u8>]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        for index_mac in index_macs {
            if let Some(mac) =
                MutationMac::filter_by_name_and_index_mac(name, hex_encode(index_mac))
                    .first()
                    .exec(&mut db)
                    .await
                    .map_err(db_err)?
            {
                mac.delete().exec(&mut db).await.map_err(db_err)?;
            }
        }

        Ok(())
    }

    async fn clear_mutation_macs(&self, name: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let macs = MutationMac::filter_by_name(name)
            .exec(&mut db)
            .await
            .map_err(db_err)?;
        for mac in macs {
            mac.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_latest_sync_key_id(&self) -> StoreResult<Option<Vec<u8>>> {
        let mut db = self.store.db().clone();
        let keys = SyncKey::all().exec(&mut db).await.map_err(db_err)?;

        // The highest auto id is the most recently inserted key (waepic: ORDER BY rowid DESC).
        Ok(keys
            .into_iter()
            .max_by_key(|key| key.id)
            .map(|key| hex_decode(&key.key_id))
            .transpose()?)
    }
}
