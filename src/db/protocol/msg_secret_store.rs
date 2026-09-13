use whatsapp_rust::{
    async_trait,
    wacore::store::{
        error::Result as StoreResult,
        traits::{
            MsgSecretEntry, MsgSecretStore, merge_msg_secret_expiry, merge_msg_secret_message_ts,
        },
    },
};

use super::{
    MsgSecret,
    backend::{ProtocolBackend, db_err},
};

#[async_trait]
impl MsgSecretStore for ProtocolBackend {
    async fn put_msg_secrets(&self, entries: Vec<MsgSecretEntry>) -> StoreResult<usize> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        for entry in &entries {
            let existing = MsgSecret::filter_by_chat_and_sender_and_msg_id(
                entry.chat.as_ref(),
                entry.sender.as_ref(),
                entry.msg_id.as_ref(),
            )
            .first()
            .exec(&mut db)
            .await
            .map_err(db_err)?;
            if let Some(mut existing) = existing {
                let merged_expires = merge_msg_secret_expiry(existing.expires_at, entry.expires_at);
                let merged_ts = merge_msg_secret_message_ts(existing.message_ts, entry.message_ts);

                existing
                    .update()
                    .secret(entry.secret.to_vec())
                    .expires_at(merged_expires)
                    .message_ts(merged_ts)
                    .exec(&mut db)
                    .await
                    .map_err(db_err)?;
            } else {
                MsgSecret::create()
                    .chat(entry.chat.to_string())
                    .sender(entry.sender.to_string())
                    .msg_id(entry.msg_id.to_string())
                    .secret(entry.secret.to_vec())
                    .expires_at(entry.expires_at)
                    .message_ts(entry.message_ts)
                    .exec(&mut db)
                    .await
                    .map_err(db_err)?;
            }
        }

        Ok(entries.len())
    }

    async fn get_msg_secret(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
    ) -> StoreResult<Option<Vec<u8>>> {
        let mut db = self.store.db().clone();
        Ok(
            MsgSecret::filter_by_chat_and_sender_and_msg_id(chat, sender, msg_id)
                .first()
                .exec(&mut db)
                .await
                .map_err(db_err)?
                .map(|entity| entity.secret),
        )
    }

    async fn get_msg_secret_with_ts(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
    ) -> StoreResult<Option<(Vec<u8>, i64)>> {
        let mut db = self.store.db().clone();
        Ok(
            MsgSecret::filter_by_chat_and_sender_and_msg_id(chat, sender, msg_id)
                .first()
                .exec(&mut db)
                .await
                .map_err(db_err)?
                .map(|entity| (entity.secret, entity.message_ts)),
        )
    }

    async fn delete_expired_msg_secrets(&self, cutoff_timestamp: i64) -> StoreResult<u32> {
        let _guard = self.store.write_lock().lock().await;
        let mut db = self.store.db().clone();
        let secrets = MsgSecret::all().exec(&mut db).await.map_err(db_err)?;
        let mut deleted = 0;
        for secret in secrets {
            // 0 means never expire.
            if secret.expires_at != 0 && secret.expires_at <= cutoff_timestamp {
                secret.delete().exec(&mut db).await.map_err(db_err)?;
                deleted += 1;
            }
        }

        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }
}
