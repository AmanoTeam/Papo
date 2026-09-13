use whatsapp_rust::{
    async_trait,
    bytes::Bytes,
    wacore::store::{
        error::{Result as StoreResult, StoreError},
        traits::SignalStore,
    },
};

use super::{
    Identity, PreKey, SenderKey, SignalSession, SignedPreKey,
    backend::{ProtocolBackend, db_err},
};

#[async_trait]
impl SignalStore for ProtocolBackend {
    async fn put_identity(&self, address: &str, key: [u8; 32]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        Identity::upsert_by_address(address)
            .key(key.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn load_identity(&self, address: &str) -> StoreResult<Option<[u8; 32]>> {
        let mut db = self.store.db().clone();
        let identity = Identity::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        match identity {
            Some(entity) => {
                let key: [u8; 32] = entity
                    .key
                    .try_into()
                    .map_err(|_| StoreError::Validation("identity key must be 32 bytes".into()))?;
                Ok(Some(key))
            }
            None => Ok(None),
        }
    }

    async fn delete_identity(&self, address: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(identity) = Identity::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            identity.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_session(&self, address: &str) -> StoreResult<Option<Bytes>> {
        let mut db = self.store.db().clone();
        Ok(SignalSession::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|entity| Bytes::from(entity.data)))
    }

    async fn put_session(&self, address: &str, session: &[u8]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        SignalSession::upsert_by_address(address)
            .data(session.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn delete_session(&self, address: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(session) = SignalSession::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            session.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn store_prekey(&self, id: u32, record: &[u8], uploaded: bool) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        PreKey::upsert_by_id(i64::from(id))
            .record(record.to_vec())
            .uploaded(uploaded)
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn load_prekey(&self, id: u32) -> StoreResult<Option<Bytes>> {
        let mut db = self.store.db().clone();
        Ok(PreKey::filter_by_id(i64::from(id))
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|prekey| Bytes::from(prekey.record)))
    }

    async fn mark_prekeys_uploaded(&self, ids: &[u32]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        for &id in ids {
            if let Some(mut prekey) = PreKey::filter_by_id(i64::from(id))
                .first()
                .exec(&mut db)
                .await
                .map_err(db_err)?
            {
                prekey
                    .update()
                    .uploaded(true)
                    .exec(&mut db)
                    .await
                    .map_err(db_err)?;
            }
        }

        Ok(())
    }

    async fn remove_prekey(&self, id: u32) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(prekey) = PreKey::filter_by_id(i64::from(id))
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            prekey.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_max_prekey_id(&self) -> StoreResult<u32> {
        let mut db = self.store.db().clone();
        let prekeys = PreKey::all().exec(&mut db).await.map_err(db_err)?;
        let max = prekeys.iter().map(|prekey| prekey.id).max().unwrap_or(0);

        Ok(u32::try_from(max).unwrap_or(u32::MAX))
    }

    async fn store_signed_prekey(&self, id: u32, record: &[u8]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        SignedPreKey::upsert_by_id(i64::from(id))
            .record(record.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn load_signed_prekey(&self, id: u32) -> StoreResult<Option<Vec<u8>>> {
        let mut db = self.store.db().clone();
        Ok(SignedPreKey::filter_by_id(i64::from(id))
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|prekey| prekey.record))
    }

    async fn load_all_signed_prekeys(&self) -> StoreResult<Vec<(u32, Vec<u8>)>> {
        let mut db = self.store.db().clone();
        let prekeys = SignedPreKey::all().exec(&mut db).await.map_err(db_err)?;

        Ok(prekeys
            .into_iter()
            .map(|prekey| (u32::try_from(prekey.id).unwrap_or(0), prekey.record))
            .collect())
    }

    async fn remove_signed_prekey(&self, id: u32) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(prekey) = SignedPreKey::filter_by_id(i64::from(id))
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            prekey.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn put_sender_key(&self, address: &str, record: &[u8]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        SenderKey::upsert_by_address(address)
            .record(record.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn get_sender_key(&self, address: &str) -> StoreResult<Option<Vec<u8>>> {
        let mut db = self.store.db().clone();
        Ok(SenderKey::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|sender_key| sender_key.record))
    }

    async fn delete_sender_key(&self, address: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(sender_key) = SenderKey::filter_by_address(address)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            sender_key.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }
}
