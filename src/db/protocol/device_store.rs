use whatsapp_rust::{
    async_trait, serde_json,
    wacore::store::{Device, error::Result as StoreResult, traits::DeviceStore},
};

use super::{
    DeviceState,
    backend::{ProtocolBackend, db_err, serde_err},
};

#[async_trait]
impl DeviceStore for ProtocolBackend {
    async fn save(&self, device: &Device) -> StoreResult<()> {
        let json = serde_json::to_string(device).map_err(serde_err)?;
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        DeviceState::upsert_by_id(0)
            .data(json)
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn load(&self) -> StoreResult<Option<Device>> {
        let mut db = self.store.db().clone();
        let state = DeviceState::filter_by_id(0)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        state
            .map(|entity| serde_json::from_str(&entity.data).map_err(serde_err))
            .transpose()
    }

    async fn exists(&self) -> StoreResult<bool> {
        let mut db = self.store.db().clone();
        Ok(DeviceState::filter_by_id(0)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .is_some())
    }

    async fn create(&self) -> StoreResult<i32> {
        let json = serde_json::to_string(&Device::new()).map_err(serde_err)?;
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if DeviceState::filter_by_id(0)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .is_none()
        {
            DeviceState::create()
                .id(0)
                .data(json)
                .exec(&mut db)
                .await
                .map_err(db_err)?;
        }

        Ok(0)
    }
}
