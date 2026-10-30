use std::{collections::HashMap, fmt, fs, sync::Arc};

use indexmap::IndexMap;

use jiff::Timestamp;
use toasty::{Db, Executor};
use toasty_core::driver::operation::{RawSql, RawSqlRet};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    db::{
        entities::{Chat as ChatEntity, Contact, Message as MessageEntity, PendingReadMark},
        media::MediaStorage,
        protocol::LidMapping,
    },
    state::{Chat, ChatMessage, ChatQuote, Media, MediaType, MessageStatus},
    utils::{cmp_chat_order, is_display_name_junk},
};

/// Store for chat, message, and contact data backed by a per-session
/// encrypted database.
///
/// Each session has its own database file containing the full chat history,
/// messages, and contacts for a single `WhatsApp` account. Media attachments
/// are stored as files on disk under `DATA_DIR/media/{session}/`, with only
/// their paths kept in the database.
///
/// Cloning the store is cheap: the underlying database handle shares a
/// connection pool, so clones can be freely handed to components.
#[derive(Clone)]
pub struct SessionStore {
    db: Db,
    session: String,
    write_lock: Arc<Mutex<()>>,
}

impl fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionStore")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl SessionStore {
    /// Creates a new store for the given session database handle.
    ///
    /// The `session` UUID is used to build media file paths under
    /// `DATA_DIR/media/{session}/{chat}/{message}.{ext}`.
    pub fn new(db: Db, session: &str) -> Self {
        Self {
            db,
            session: session.to_string(),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub(crate) fn write_lock(&self) -> &Arc<Mutex<()>> {
        &self.write_lock
    }

    fn media_storage(&self) -> MediaStorage {
        MediaStorage::new(&self.session)
    }

    fn chat_from_entity(entity: ChatEntity) -> Chat {
        Chat {
            jid: entity.jid,
            name: entity.name,
            muted: entity.muted,
            pinned: entity.pinned,
            pinned_at: entity.pinned_at,
            archived: entity.archived,
            available: None,
            last_seen: None,
            avatar_path: None,
            participants: HashMap::new(),
            member_count: None,
            last_message_time: entity.last_message_time.unwrap_or(Timestamp::MIN),
        }
    }

    /// Converts a stored message entity into the runtime `ChatMessage`,
    /// loading media bytes from disk when a media path is present, or
    /// building a metadata-only placeholder when only `media_type` is set.
    fn message_from_entity(entity: MessageEntity) -> ChatMessage {
        let media = if let Some(data) = entity
            .media_path
            .as_ref()
            .and_then(|path| MediaStorage::load_media(path).ok())
        {
            Some(Media {
                data: Arc::new(data),
                mime_type: entity.media_mime.clone().unwrap_or_default(),
                r#type: entity
                    .media_type
                    .as_deref()
                    .map_or(MediaType::Image, |t| MediaType::from(t.to_string())),
                ..Media::default()
            })
        } else {
            entity.media_type.as_deref().map(|t| Media {
                r#type: MediaType::from(t),
                mime_type: entity.media_mime.clone().unwrap_or_default(),
                ..Media::default()
            })
        };

        let quote =
            (entity.quote_body.is_some() || entity.quote_media.is_some()).then(|| ChatQuote {
                body: entity.quote_body.clone(),
                media: entity.quote_media.clone(),
                stanza_id: entity.quote_stanza_id.clone(),
                participant: entity.quote_participant.clone(),
                sender_name: entity.quote_sender_name.clone(),
                sender_push: false,
            });

        ChatMessage {
            media,
            quote,
            status: MessageStatus::from(i32::try_from(entity.status).unwrap_or_default()),
            content: entity.content.unwrap_or_default(),
            chat_jid: entity.chat_jid,
            local_id: Uuid::parse_str(&entity.local_id).unwrap_or_else(|_| Uuid::new_v4()),
            outgoing: entity.outgoing,
            reactions: IndexMap::new(),
            sender_jid: entity.sender_jid,
            server_id: entity.server_id.unwrap_or_default(),
            timestamp: entity.timestamp,
            sender_name: entity.sender_name,
            sender_push: false,
        }
    }

    /// One grouped pass over the list of chats: each chat's newest
    /// message and its unread count together, replacing the per-chat
    /// queries populate previously ran row by row.
    pub async fn load_chat_preview_snapshot(
        &self,
    ) -> Result<HashMap<String, (Option<ChatMessage>, u32)>, toasty::Error> {
        Ok(HashMap::new())
    }
}

// ── Chat operations ───────────────────────────────────────────

impl SessionStore {
    /// Upserts a chat. If a chat with the same `JID` exists, its name,
    /// muted, pinned, archived, and `last_message_time` fields are updated.
    pub async fn save_chat(&self, chat: &Chat) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        ChatEntity::upsert_by_jid(chat.jid.clone())
            .name(chat.name.clone())
            .muted(chat.muted)
            .pinned(chat.pinned)
            .pinned_at(chat.pinned_at)
            .archived(chat.archived)
            .last_message_time(Some(chat.last_message_time))
            .exec(&mut db)
            .await?;

        Ok(())
    }

    /// Inserts a chat row if it does not already exist (placeholder for `FK`).
    pub async fn ensure_chat_exists(&self, jid: &str) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        self.ensure_chat_exists_locked(jid).await
    }

    /// Inserts a chat row if one does not already exist, without locking.
    ///
    /// Callers must already hold the write lock.
    async fn ensure_chat_exists_locked(&self, jid: &str) -> Result<(), toasty::Error> {
        let mut db = self.db.clone();
        if ChatEntity::filter_by_jid(jid)
            .first()
            .exec(&mut db)
            .await?
            .is_none()
        {
            ChatEntity::create()
                .jid(jid.to_string())
                .name(jid.to_string())
                .muted(false)
                .pinned(false)
                .pinned_at(None)
                .archived(false)
                .last_message_time(None)
                .exec(&mut db)
                .await?;
        }

        Ok(())
    }

    /// Loads a single non-archived chat by `JID`.
    pub async fn load_chat(&self, jid: &str) -> Result<Option<Chat>, toasty::Error> {
        let mut db = self.db.clone();
        Ok(ChatEntity::filter_by_jid(jid)
            .first()
            .exec(&mut db)
            .await?
            .filter(|entity| !entity.archived)
            .map(Self::chat_from_entity))
    }

    /// Loads all non-archived chats, ordered by pinned, then pin time,
    /// then `last_message_time` — newest pin on top, reference order.
    ///
    /// The sort key is repaired against the messages table on the way
    /// out: backfilled history once regressed it toward older chunks,
    /// and only the true newest message may order the list.
    pub async fn load_chats(&self) -> Result<Vec<Chat>, toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        let mut chats = ChatEntity::filter_by_archived(false).exec(&mut db).await?;

        for chat in &mut chats {
            let Some(newest) = MessageEntity::filter_by_chat_jid(chat.jid.as_str())
                .order_by(MessageEntity::fields().timestamp().desc())
                .limit(1)
                .exec(&mut db)
                .await?
                .into_iter()
                .next()
                .map(|message| message.timestamp)
            else {
                continue;
            };

            if chat.last_message_time.is_none_or(|stored| newest > stored) {
                chat.update()
                    .last_message_time(Some(newest))
                    .exec(&mut db)
                    .await?;
                chat.last_message_time = Some(newest);
            }
        }

        chats.sort_by(|a, b| {
            cmp_chat_order(
                &(
                    a.pinned,
                    a.pinned_at,
                    a.last_message_time.unwrap_or(Timestamp::MIN),
                ),
                &(
                    b.pinned,
                    b.pinned_at,
                    b.last_message_time.unwrap_or(Timestamp::MIN),
                ),
            )
        });

        Ok(chats
            .into_iter()
            .map(Self::chat_from_entity)
            .collect::<Vec<_>>())
    }

    /// Deletes a chat and all its messages (cascade via `FK` relationship).
    pub async fn delete_chat(&self, jid: &str) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        if let Some(chat) = ChatEntity::filter_by_jid(jid).first().exec(&mut db).await? {
            chat.delete().exec(&mut db).await?;
        }

        self.media_storage().delete_chat_media(jid);

        Ok(())
    }

    /// Marks all pending or delivered messages in a chat as read.
    /// Marks a chat's messages as read, returning the flipped messages'
    /// `local_id`s. Only incoming messages are flipped, unless the chat is the
    /// user's own, where outgoing messages are also incoming to self.
    pub async fn mark_chat_read(
        &self,
        chat_jid: &str,
        own_chat: bool,
    ) -> Result<Vec<Uuid>, toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        let mut query = MessageEntity::filter_by_chat_jid(chat_jid);
        if !own_chat {
            query = query.filter(MessageEntity::fields().outgoing().eq(false));
        }
        let messages = query.exec(&mut db).await?;

        let mut flipped = Vec::new();
        for mut message in messages {
            if message.status != 1 && message.status != 2 {
                message.update().status(1).exec(&mut db).await?;
                flipped.push(Uuid::parse_str(&message.local_id).unwrap_or_else(|_| Uuid::new_v4()));
            }
        }

        Ok(flipped)
    }
}

// ── Message operations ──────────────────────────────────────────

impl SessionStore {
    /// Upserts a message by `local_id` and updates the chat's `last_message_time`.
    pub async fn save_message(
        &self,
        chat_jid: &str,
        msg: &ChatMessage,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        // A save may advance the lifecycle (or record a `Failed` send)
        // but must never regress it.
        let existing_status = MessageEntity::filter_by_local_id(msg.local_id.to_string())
            .first()
            .exec(&mut db)
            .await?
            .filter(|entity| entity.chat_jid == chat_jid)
            .map(|entity| MessageStatus::from(i32::try_from(entity.status).unwrap_or_default()));
        let status = match existing_status {
            Some(existing)
                if msg.status != MessageStatus::Failed
                    && msg.status.stage() <= existing.stage() =>
            {
                existing
            }
            _ => msg.status,
        };

        MessageEntity::upsert_by_local_id(msg.local_id.to_string())
            .server_id(if msg.server_id.is_empty() {
                None
            } else {
                Some(msg.server_id.clone())
            })
            .chat_jid(chat_jid.to_string())
            .sender_jid(msg.sender_jid.clone())
            .sender_name(msg.sender_name.clone().filter(|n| !is_display_name_junk(n)))
            .content(if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            })
            .outgoing(msg.outgoing)
            .status(i64::from(status as u8))
            .timestamp(msg.timestamp)
            .media_type(msg.media.as_ref().map(|m| format!("{:?}", m.r#type)))
            .media_path(self.save_media_file(chat_jid, msg))
            .media_mime(msg.media.as_ref().map(|m| m.mime_type.clone()))
            .quote_body(msg.quote.as_ref().and_then(|quote| quote.body.clone()))
            .quote_media(msg.quote.as_ref().and_then(|quote| quote.media.clone()))
            .quote_stanza_id(msg.quote.as_ref().and_then(|quote| quote.stanza_id.clone()))
            .quote_participant(
                msg.quote
                    .as_ref()
                    .and_then(|quote| quote.participant.clone()),
            )
            .quote_sender_name(
                msg.quote
                    .as_ref()
                    .and_then(|quote| quote.sender_name.clone())
                    .filter(|n| !is_display_name_junk(n)),
            )
            .exec(&mut db)
            .await?;

        if let Some(mut chat) = ChatEntity::filter_by_jid(chat_jid)
            .first()
            .exec(&mut db)
            .await?
            && chat
                .last_message_time
                .is_none_or(|current| msg.timestamp > current)
        {
            chat.update()
                .last_message_time(Some(msg.timestamp))
                .exec(&mut db)
                .await?;
        }

        Ok(())
    }

    /// Queues (or refreshes) a durable chat read mark for the multi-device
    /// appstate sync. Re-queueing pulls the retry deadline forward so a
    /// fresh send is attempted promptly.
    pub async fn queue_read_mark(
        &self,
        chat_jid: &str,
        position: i64,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        let now = Timestamp::now().as_second();

        if let Some(mut existing) = PendingReadMark::filter_by_chat_jid(chat_jid.to_owned())
            .first()
            .exec(&mut db)
            .await?
        {
            existing
                .update()
                .position(position)
                .next_retry_at(now)
                .exec(&mut db)
                .await?;
        } else {
            PendingReadMark::create()
                .chat_jid(chat_jid.to_owned())
                .position(position)
                .attempts(0)
                .next_retry_at(now)
                .exec(&mut db)
                .await?;
        }

        Ok(())
    }

    /// The next queued read mark whose retry deadline has passed. The
    /// queue is one row per chat, so the scan is trivial.
    pub async fn next_due_read_mark(
        &self,
        now: i64,
    ) -> Result<Option<PendingReadMark>, toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        let marks = PendingReadMark::all().exec(&mut db).await?;
        Ok(marks
            .into_iter()
            .filter(|mark| mark.next_retry_at <= now)
            .min_by_key(|mark| (mark.next_retry_at, mark.position)))
    }

    /// Removes a read mark whose sync went through.
    pub async fn delete_read_mark(&self, chat_jid: &str) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        if let Some(mark) = PendingReadMark::filter_by_chat_jid(chat_jid.to_owned())
            .first()
            .exec(&mut db)
            .await?
        {
            mark.delete().exec(&mut db).await?;
        }

        Ok(())
    }

    /// Records a failed sync attempt and sets the next retry deadline.
    pub async fn defer_read_mark(
        &self,
        chat_jid: &str,
        attempts: i64,
        next_retry_at: i64,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        if let Some(mut mark) = PendingReadMark::filter_by_chat_jid(chat_jid.to_owned())
            .first()
            .exec(&mut db)
            .await?
        {
            mark.update()
                .attempts(attempts)
                .next_retry_at(next_retry_at)
                .exec(&mut db)
                .await?;
        }

        Ok(())
    }

    /// Persists freshly resolved names back onto a stored message,
    /// sender and quoted-sender alike. Callers write only when the
    /// resolution beat the saved value, so each row converges to its
    /// best known name instead of being re-derived on every load.
    pub async fn update_message_names(
        &self,
        local_id: &str,
        sender_name: Option<&str>,
        quote_sender_name: Option<&str>,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        let sanitize =
            |name: Option<&str>| name.map(str::to_owned).filter(|n| !is_display_name_junk(n));

        if let Some(mut message) = MessageEntity::filter_by_local_id(local_id.to_owned())
            .first()
            .exec(&mut db)
            .await?
        {
            message
                .update()
                .sender_name(sanitize(sender_name))
                .quote_sender_name(sanitize(quote_sender_name))
                .exec(&mut db)
                .await?;
        }

        Ok(())
    }

    /// Clears the push-name marker older builds baked into stored
    /// sender columns: the tilde is painted at render time and data
    /// stays canonical.
    pub async fn strip_tilde_markers(&self) {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        for column in ["sender_name", "quote_sender_name"] {
            let result = db
                .exec_raw_sql(RawSql {
                    sql: format!(
                        r#"UPDATE "messages" SET "{column}" = SUBSTR("{column}", 2) WHERE "{column}" LIKE '~%'"#
                    ),
                    ret: RawSqlRet::None,
                    params: Vec::new(),
                })
                .await;

            if let Err(e) = result {
                tracing::warn!("Failed to strip tilde markers from {column}: {e}");
            }
        }
    }

    /// Inserts a message if its `server_id` does not already exist.
    /// Returns `true` if a new row was inserted, `false` if it was a duplicate.
    pub async fn save_synced_message(
        &self,
        chat_jid: &str,
        msg: &ChatMessage,
    ) -> Result<bool, toasty::Error> {
        let _guard = self.write_lock.lock().await;
        self.ensure_chat_exists_locked(chat_jid).await?;

        let mut db = self.db.clone();
        if !msg.server_id.is_empty()
            && let Some(mut existing) =
                MessageEntity::filter_by_server_id(Some(msg.server_id.clone()))
                    .first()
                    .exec(&mut db)
                    .await?
        {
            let existing_status =
                MessageStatus::from(i32::try_from(existing.status).unwrap_or_default());
            if msg.status != MessageStatus::Failed && msg.status.stage() > existing_status.stage() {
                existing
                    .update()
                    .status(i64::from(msg.status as u8))
                    .exec(&mut db)
                    .await?;
            }

            return Ok(false);
        }

        MessageEntity::create()
            .local_id(msg.local_id.to_string())
            .server_id(if msg.server_id.is_empty() {
                None
            } else {
                Some(msg.server_id.clone())
            })
            .chat_jid(chat_jid.to_string())
            .sender_jid(msg.sender_jid.clone())
            .sender_name(msg.sender_name.clone().filter(|n| !is_display_name_junk(n)))
            .content(if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            })
            .outgoing(msg.outgoing)
            .status(i64::from(msg.status as u8))
            .timestamp(msg.timestamp)
            .media_type(msg.media.as_ref().map(|m| format!("{:?}", m.r#type)))
            .media_path(self.save_media_file(chat_jid, msg))
            .media_mime(msg.media.as_ref().map(|m| m.mime_type.clone()))
            .quote_body(msg.quote.as_ref().and_then(|quote| quote.body.clone()))
            .quote_media(msg.quote.as_ref().and_then(|quote| quote.media.clone()))
            .quote_stanza_id(msg.quote.as_ref().and_then(|quote| quote.stanza_id.clone()))
            .quote_participant(
                msg.quote
                    .as_ref()
                    .and_then(|quote| quote.participant.clone()),
            )
            .quote_sender_name(
                msg.quote
                    .as_ref()
                    .and_then(|quote| quote.sender_name.clone())
                    .filter(|n| !is_display_name_junk(n)),
            )
            .exec(&mut db)
            .await?;

        if let Some(mut chat) = ChatEntity::filter_by_jid(chat_jid)
            .first()
            .exec(&mut db)
            .await?
            && chat
                .last_message_time
                .is_none_or(|current| msg.timestamp > current)
        {
            chat.update()
                .last_message_time(Some(msg.timestamp))
                .exec(&mut db)
                .await?;
        }

        Ok(true)
    }

    pub async fn load_message_by_local_id(
        &self,
        chat_jid: &str,
        msg_id: &Uuid,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        let mut db = self.db.clone();
        Ok(MessageEntity::filter_by_local_id(msg_id.to_string())
            .first()
            .exec(&mut db)
            .await?
            .filter(|m| m.chat_jid == chat_jid)
            .map(Self::message_from_entity))
    }

    pub async fn load_message_by_server_id(
        &self,
        chat_jid: &str,
        msg_id: &str,
    ) -> Result<Option<ChatMessage>, toasty::Error> {
        let mut db = self.db.clone();
        Ok(MessageEntity::filter_by_server_id(if msg_id.is_empty() {
            None
        } else {
            Some(msg_id.to_string())
        })
        .first()
        .exec(&mut db)
        .await?
        .filter(|m| m.chat_jid == chat_jid)
        .map(Self::message_from_entity))
    }

    pub async fn lid_to_pn_jid(&self, lid_jid: &str) -> Option<String> {
        let lid = lid_jid.strip_suffix("@lid")?;
        let mut db = self.db.clone();

        LidMapping::filter_by_lid(lid.to_string())
            .first()
            .exec(&mut db)
            .await
            .ok()?
            .map(|entry| format!("{}@s.whatsapp.net", entry.phone_number))
    }

    pub async fn pn_to_lid_jids(&self, pn_jid: &str) -> Vec<String> {
        let phone = pn_jid.split('@').next().unwrap_or(pn_jid);
        let mut db = self.db.clone();
        let Ok(mappings) = LidMapping::all().exec(&mut db).await else {
            return Vec::new();
        };

        mappings
            .into_iter()
            .filter(|m| m.phone_number == phone)
            .map(|m| format!("{}@lid", m.lid))
            .collect()
    }

    pub async fn learn_lid_mapping(
        &self,
        lid: &str,
        phone_number: &str,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;

        // Callers pass either bare digits or full jids; the table is keyed
        // by the bare local part on both sides (see `lid_to_pn_jid` and
        // `pn_to_lid_jids`), so normalize here no matter the input form.
        let lid = lid.split('@').next().unwrap_or_default();
        let phone_number = phone_number.split('@').next().unwrap_or_default();
        if lid.is_empty() || phone_number.is_empty() {
            return Ok(());
        }

        let mut db = self.db.clone();
        let now = Timestamp::now().as_millisecond();

        LidMapping::upsert_by_lid(lid)
            .phone_number(phone_number.to_string())
            .created_at(now)
            .updated_at(now)
            .learning_source("history_sync".to_string())
            .exec(&mut db)
            .await?;

        Ok(())
    }

    /// Every mapping row, as bare `(lid, phone)` digit pairs. Used for
    /// load-time sender resolution, where several per-sender lookups
    /// would turn into a query storm.
    pub async fn get_all_lid_mappings(&self) -> Vec<(String, String)> {
        let mut db = self.db.clone();
        match LidMapping::all().exec(&mut db).await {
            Ok(mappings) => mappings
                .into_iter()
                .map(|entry| (entry.lid, entry.phone_number))
                .collect(),
            Err(e) => {
                tracing::error!("Failed to load lid mappings: {e}");
                Vec::new()
            }
        }
    }

    /// Loads messages for a chat, most recent first, up to `limit`.
    pub async fn load_messages(
        &self,
        chat_jid: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        let mut db = self.db.clone();
        let messages = MessageEntity::filter_by_chat_jid(chat_jid)
            .order_by(MessageEntity::fields().timestamp().desc())
            .limit(limit)
            .exec(&mut db)
            .await?;

        Ok(messages
            .into_iter()
            .map(Self::message_from_entity)
            .collect::<Vec<_>>())
    }

    /// Loads messages after a timestamp, oldest first, up to `limit`.
    pub async fn load_messages_after(
        &self,
        chat_jid: &str,
        after_timestamp: Timestamp,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        let mut db = self.db.clone();
        let messages = MessageEntity::filter_by_chat_jid(chat_jid)
            .filter(MessageEntity::fields().timestamp().gt(after_timestamp))
            .order_by(MessageEntity::fields().timestamp().asc())
            .limit(limit)
            .exec(&mut db)
            .await?;

        Ok(messages
            .into_iter()
            .map(Self::message_from_entity)
            .collect::<Vec<_>>())
    }

    /// Loads messages before a timestamp, newest first, up to `limit`.
    pub async fn load_messages_before(
        &self,
        chat_jid: &str,
        before_timestamp: Timestamp,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        let mut db = self.db.clone();
        let messages = MessageEntity::filter_by_chat_jid(chat_jid)
            .filter(MessageEntity::fields().timestamp().lt(before_timestamp))
            .order_by(MessageEntity::fields().timestamp().desc())
            .limit(limit)
            .exec(&mut db)
            .await?;

        Ok(messages
            .into_iter()
            .map(Self::message_from_entity)
            .collect::<Vec<_>>())
    }

    pub async fn delete_message(&self, server_id: &str) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        if let Some(msg) = MessageEntity::filter_by_server_id(if server_id.is_empty() {
            None
        } else {
            Some(server_id.to_string())
        })
        .first()
        .exec(&mut db)
        .await?
        {
            if let Some(path) = &msg.media_path {
                let _ = fs::remove_file(path);
            }

            msg.delete().exec(&mut db).await?;
        }

        Ok(())
    }

    pub async fn set_message_status(
        &self,
        local_id: Uuid,
        status: MessageStatus,
    ) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        let mut message = MessageEntity::filter_by_local_id(local_id.to_string())
            .one()
            .exec(&mut db)
            .await?;
        message.update().status(status as i64).exec(&mut db).await?;

        Ok(())
    }

    /// Counts unread messages in a chat (status != `Read` and not outgoing).
    pub async fn get_unread_count(&self, chat_jid: &str) -> Result<usize, toasty::Error> {
        let mut db = self.db.clone();
        MessageEntity::filter_by_chat_jid(chat_jid)
            .filter(MessageEntity::fields().status().ne(1))
            .filter(MessageEntity::fields().outgoing().eq(false))
            .exec(&mut db)
            .await
            .map(|messages| messages.len())
    }

    /// Returns all unread message entities in a chat (status != `Read`, not outgoing).
    async fn unread_entities(&self, chat_jid: &str) -> Result<Vec<MessageEntity>, toasty::Error> {
        let mut db = self.db.clone();
        MessageEntity::filter_by_chat_jid(chat_jid)
            .filter(MessageEntity::fields().status().ne(1))
            .filter(MessageEntity::fields().outgoing().eq(false))
            .exec(&mut db)
            .await
    }

    /// Returns all unread messages in a chat (status != `Read` and not outgoing).
    pub async fn get_unread_messages(
        &self,
        chat_jid: &str,
    ) -> Result<Vec<ChatMessage>, toasty::Error> {
        Ok(self
            .unread_entities(chat_jid)
            .await?
            .into_iter()
            .map(Self::message_from_entity)
            .collect::<Vec<_>>())
    }

    /// Saves a media attachment to disk and returns its relative path.
    ///
    /// Returns `None` if the message has no media or the media has no data.
    fn save_media_file(&self, chat_jid: &str, msg: &ChatMessage) -> Option<String> {
        let media = msg.media.as_ref()?;
        if media.data.is_empty() {
            return None;
        }

        let ext = media_type_extension(media.r#type);
        let storage = self.media_storage();
        storage
            .save_media(chat_jid, &msg.local_id.to_string(), ext, &media.data)
            .ok()
    }
}

// ── Contact operations ──────────────────────────────────────────

impl SessionStore {
    pub async fn save_contact(&self, contact: &Contact) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();

        let existing = Contact::filter_by_jid(contact.jid.clone())
            .first()
            .exec(&mut db)
            .await?;
        let (existing_name, existing_push) = existing
            .map(|row| (row.name, row.push_name))
            .unwrap_or_default();

        let name = contact
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .or(existing_name);
        let push_name = contact
            .push_name
            .clone()
            .filter(|push| !push.is_empty())
            .or(existing_push);

        Contact::upsert_by_jid(contact.jid.clone())
            .name(name)
            .push_name(push_name)
            .phone_number(contact.phone_number.clone())
            .is_registered(contact.is_registered)
            .last_updated(Timestamp::now().as_second())
            .profile_picture_url(contact.profile_picture_url.clone())
            .exec(&mut db)
            .await?;

        Ok(())
    }

    pub async fn get_contact(&self, jid: &str) -> Result<Option<Contact>, toasty::Error> {
        let mut db = self.db.clone();
        Contact::filter_by_jid(jid).first().exec(&mut db).await
    }

    pub async fn get_all_contacts(&self) -> Result<Vec<Contact>, toasty::Error> {
        let mut db = self.db.clone();
        let mut contacts = Contact::all().exec(&mut db).await?;

        contacts.sort_by(|a, b| {
            a.name
                .clone()
                .unwrap_or_default()
                .cmp(&b.name.clone().unwrap_or_default())
        });

        Ok(contacts)
    }
}

// ── Search operations ──────────────────────────────────────────

impl SessionStore {
    /// Searches contacts by name, `push_name`, or `JID` (case-insensitive).
    pub async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>, toasty::Error> {
        let mut db = self.db.clone();

        Ok(Contact::filter(
            Contact::fields()
                .name()
                .like(format!("%{query}%"))
                .or(Contact::fields().push_name().like(format!("%{query}%")))
                .or(Contact::fields().jid().like(format!("%{query}%"))),
        )
        .exec(&mut db)
        .await?
        .into_iter()
        .collect::<Vec<_>>())
    }

    /// Searches messages by content (case-insensitive), most recent first.
    pub async fn search_messages(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, ChatMessage)>, toasty::Error> {
        let mut db = self.db.clone();

        Ok(
            MessageEntity::filter(MessageEntity::fields().content().like(format!("%{query}%")))
                .order_by(MessageEntity::fields().timestamp().desc())
                .limit(limit as usize)
                .exec(&mut db)
                .await?
                .into_iter()
                .map(|entity| (entity.chat_jid.clone(), Self::message_from_entity(entity)))
                .collect::<Vec<_>>(),
        )
    }
}

fn media_type_extension(media_type: MediaType) -> &'static str {
    match media_type {
        MediaType::Audio => "ogg",
        MediaType::Image => "jpg",
        MediaType::Video => "mp4",
        MediaType::Sticker => "webp",
        MediaType::Document => "bin",
    }
}
