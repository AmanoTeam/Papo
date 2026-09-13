use jiff::Timestamp;

use whatsapp_rust::{
    async_trait, serde_json,
    wacore::store::{
        error::Result as StoreResult,
        traits::{DeviceListRecord, LidPnMappingEntry, ProtocolStore, TcTokenEntry},
    },
};

use super::{
    BaseKey, DeviceList, LidMapping, SenderKeyDevice, SentMessage, TcToken,
    backend::{ProtocolBackend, db_err, serde_err},
};

fn lid_mapping_entry(entity: LidMapping) -> LidPnMappingEntry {
    LidPnMappingEntry {
        lid: entity.lid,
        phone_number: entity.phone_number,
        created_at: entity.created_at,
        updated_at: entity.updated_at,
        learning_source: entity.learning_source,
    }
}

#[async_trait]
impl ProtocolStore for ProtocolBackend {
    async fn get_sender_key_devices(&self, group_jid: &str) -> StoreResult<Vec<(String, bool)>> {
        let mut db = self.store.db().clone();
        Ok(SenderKeyDevice::filter_by_group_jid(group_jid)
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .into_iter()
            .map(|device| (device.device_jid, device.has_key))
            .collect())
    }

    async fn set_sender_key_status(
        &self,
        group_jid: &str,
        entries: &[(&str, bool)],
    ) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        for &(device_jid, has_key) in entries {
            SenderKeyDevice::upsert_by_group_jid_and_device_jid(group_jid, device_jid)
                .has_key(has_key)
                .exec(&mut db)
                .await
                .map_err(db_err)?;
        }

        Ok(())
    }

    async fn clear_sender_key_devices(&self, group_jid: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let devices = SenderKeyDevice::filter_by_group_jid(group_jid)
            .exec(&mut db)
            .await
            .map_err(db_err)?;
        for device in devices {
            device.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn delete_sender_key_device_rows(&self, device_jids: &[&str]) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let devices = SenderKeyDevice::all().exec(&mut db).await.map_err(db_err)?;
        for device in devices {
            if device_jids.contains(&device.device_jid.as_str()) {
                device.delete().exec(&mut db).await.map_err(db_err)?;
            }
        }

        Ok(())
    }

    async fn clear_all_sender_key_devices(&self) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let devices = SenderKeyDevice::all().exec(&mut db).await.map_err(db_err)?;
        for device in devices {
            device.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_lid_mapping(&self, lid: &str) -> StoreResult<Option<LidPnMappingEntry>> {
        let mut db = self.store.db().clone();
        Ok(LidMapping::filter_by_lid(lid)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(lid_mapping_entry))
    }

    async fn get_pn_mapping(&self, phone: &str) -> StoreResult<Option<LidPnMappingEntry>> {
        let mut db = self.store.db().clone();
        let mappings = LidMapping::all().exec(&mut db).await.map_err(db_err)?;

        Ok(mappings
            .into_iter()
            .filter(|mapping| mapping.phone_number == phone)
            .max_by_key(|mapping| mapping.updated_at)
            .map(lid_mapping_entry))
    }

    async fn put_lid_mapping(&self, entry: &LidPnMappingEntry) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        LidMapping::upsert_by_lid(&entry.lid)
            .phone_number(entry.phone_number.clone())
            .created_at(entry.created_at)
            .updated_at(entry.updated_at)
            .learning_source(entry.learning_source.clone())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn get_all_lid_mappings(&self) -> StoreResult<Vec<LidPnMappingEntry>> {
        let mut db = self.store.db().clone();
        let mappings = LidMapping::all().exec(&mut db).await.map_err(db_err)?;

        Ok(mappings.into_iter().map(lid_mapping_entry).collect())
    }

    async fn save_base_key(
        &self,
        address: &str,
        message_id: &str,
        base_key: &[u8],
    ) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        BaseKey::upsert_by_address_and_message_id(address, message_id)
            .key(base_key.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn has_same_base_key(
        &self,
        address: &str,
        message_id: &str,
        current_base_key: &[u8],
    ) -> StoreResult<bool> {
        let mut db = self.store.db().clone();
        Ok(
            BaseKey::filter_by_address_and_message_id(address, message_id)
                .first()
                .exec(&mut db)
                .await
                .map_err(db_err)?
                .is_some_and(|entity| entity.key == current_base_key),
        )
    }

    async fn delete_base_key(&self, address: &str, message_id: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(base_key) = BaseKey::filter_by_address_and_message_id(address, message_id)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            base_key.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn update_device_list(&self, record: DeviceListRecord) -> StoreResult<()> {
        let json = serde_json::to_string(&record).map_err(serde_err)?;
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        DeviceList::upsert_by_user(record.user.as_ref())
            .record(json)
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn get_devices(&self, user: &str) -> StoreResult<Option<DeviceListRecord>> {
        let mut db = self.store.db().clone();
        let record = DeviceList::filter_by_user(user)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        record
            .map(|entity| serde_json::from_str(&entity.record).map_err(serde_err))
            .transpose()
    }

    async fn delete_devices(&self, user: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(device_list) = DeviceList::filter_by_user(user)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            device_list.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_tc_token(&self, jid: &str) -> StoreResult<Option<TcTokenEntry>> {
        let mut db = self.store.db().clone();
        Ok(TcToken::filter_by_jid(jid)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
            .map(|entity| TcTokenEntry {
                token: entity.token,
                token_timestamp: entity.token_timestamp,
                sender_timestamp: entity.sender_timestamp,
            }))
    }

    async fn put_tc_token(&self, jid: &str, entry: &TcTokenEntry) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        TcToken::upsert_by_jid(jid)
            .token(entry.token.clone())
            .token_timestamp(entry.token_timestamp)
            .sender_timestamp(entry.sender_timestamp)
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn delete_tc_token(&self, jid: &str) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        if let Some(tc_token) = TcToken::filter_by_jid(jid)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?
        {
            tc_token.delete().exec(&mut db).await.map_err(db_err)?;
        }

        Ok(())
    }

    async fn get_all_tc_token_jids(&self) -> StoreResult<Vec<String>> {
        let mut db = self.store.db().clone();
        let tokens = TcToken::all().exec(&mut db).await.map_err(db_err)?;

        Ok(tokens.into_iter().map(|token| token.jid).collect())
    }

    async fn delete_expired_tc_tokens(
        &self,
        token_cutoff: i64,
        sender_cutoff: i64,
    ) -> StoreResult<u32> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let tokens = TcToken::all().exec(&mut db).await.map_err(db_err)?;
        let mut deleted = 0;
        for token in tokens {
            // The trait contract removes a row only when BOTH the received
            // token and the sender bucket are expired-or-absent.
            let received_expired = token.token.is_empty() || token.token_timestamp < token_cutoff;
            let sender_expired = token.sender_timestamp.is_none_or(|ts| ts < sender_cutoff);
            if received_expired && sender_expired {
                token.delete().exec(&mut db).await.map_err(db_err)?;
                deleted += 1;
            }
        }

        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }

    async fn store_sent_message(
        &self,
        chat_jid: &str,
        message_id: &str,
        payload: &[u8],
    ) -> StoreResult<()> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        SentMessage::upsert_by_chat_jid_and_message_id(chat_jid, message_id)
            .created_at(Timestamp::now().as_second())
            .payload(payload.to_vec())
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        Ok(())
    }

    async fn take_sent_message(
        &self,
        chat_jid: &str,
        message_id: &str,
    ) -> StoreResult<Option<Vec<u8>>> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let message = SentMessage::filter_by_chat_jid_and_message_id(chat_jid, message_id)
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;

        if let Some(message) = message {
            let payload = message.payload.clone();
            message.delete().exec(&mut db).await.map_err(db_err)?;
            Ok(Some(payload))
        } else {
            Ok(None)
        }
    }

    async fn delete_expired_sent_messages(&self, cutoff_timestamp: i64) -> StoreResult<u32> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let messages = SentMessage::all().exec(&mut db).await.map_err(db_err)?;
        let mut deleted = 0;
        for message in messages {
            if message.created_at < cutoff_timestamp {
                message.delete().exec(&mut db).await.map_err(db_err)?;
                deleted += 1;
            }
        }

        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }
}
