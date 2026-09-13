use std::{cmp, collections::HashMap, fmt, fs, sync::Arc};

use indexmap::IndexMap;

use jiff::Timestamp;
use toasty::Db;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    db::{
        entities::{Chat as ChatEntity, Contact, Message as MessageEntity},
        media::MediaStorage,
    },
    state::{Chat, ChatMessage, Media, MediaType, MessageStatus},
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

    fn media_storage(&self) -> MediaStorage {
        MediaStorage::new(&self.session)
    }

    fn chat_from_entity(&self, entity: ChatEntity) -> Chat {
        Chat {
            archived: entity.archived,
            available: None,
            avatar_path: None,
            db: self.clone(),
            jid: entity.jid,
            last_message_time: entity
                .last_message_time
                .and_then(|t| Timestamp::from_second(t).ok())
                .unwrap_or_else(Timestamp::now),
            last_seen: None,
            muted: entity.muted,
            name: entity.name,
            participants: HashMap::new(),
            pinned: entity.pinned,
        }
    }

    /// Converts a stored message entity into the runtime `ChatMessage`,
    /// loading media bytes from disk when a media path is present.
    fn message_from_entity(&self, entity: MessageEntity) -> ChatMessage {
        let media = entity
            .media_path
            .as_ref()
            .and_then(|path| MediaStorage::load_media(path).ok())
            .map(|data| Media {
                data: Arc::new(data),
                mime_type: entity.media_mime.clone().unwrap_or_default(),
                r#type: entity
                    .media_type
                    .as_deref()
                    .map_or(MediaType::Image, |t| MediaType::from(t.to_string())),
                ..Media::default()
            });

        ChatMessage {
            chat_jid: entity.chat_jid,
            content: entity.content.unwrap_or_default(),
            db: self.clone(),
            local_id: Uuid::parse_str(&entity.local_id).unwrap_or_else(|_| Uuid::new_v4()),
            media,
            outgoing: entity.outgoing,
            reactions: IndexMap::new(),
            sender_jid: entity.sender_jid,
            sender_name: entity.sender_name,
            server_id: entity.server_id.unwrap_or_default(),
            status: MessageStatus::from(i32::try_from(entity.status).unwrap_or_default()),
            timestamp: Timestamp::from_second(entity.timestamp)
                .unwrap_or_else(|_| Timestamp::now()),
        }
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
            .archived(chat.archived)
            .last_message_time(Some(chat.last_message_time.as_second()))
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
            .map(|entity| self.chat_from_entity(entity)))
    }

    /// Loads all non-archived chats, ordered by pinned then `last_message_time`.
    pub async fn load_chats(&self) -> Result<Vec<Chat>, toasty::Error> {
        let mut db = self.db.clone();
        let mut chats = ChatEntity::filter_by_archived(false).exec(&mut db).await?;
        chats.sort_by_key(|c| {
            (
                cmp::Reverse(c.pinned),
                cmp::Reverse(c.last_message_time.unwrap_or(i64::MIN)),
            )
        });

        Ok(chats
            .into_iter()
            .map(|entity| self.chat_from_entity(entity))
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
    pub async fn mark_chat_read(&self, chat_jid: &str) -> Result<(), toasty::Error> {
        let _guard = self.write_lock.lock().await;
        let mut db = self.db.clone();
        let messages = MessageEntity::filter_by_chat_jid(chat_jid)
            .exec(&mut db)
            .await?;

        for mut message in messages {
            if message.status == 0 || message.status == 5 {
                message.update().status(1).exec(&mut db).await?;
            }
        }

        Ok(())
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
        MessageEntity::upsert_by_local_id(msg.local_id.to_string())
            .server_id(if msg.server_id.is_empty() {
                None
            } else {
                Some(msg.server_id.clone())
            })
            .chat_jid(chat_jid.to_string())
            .sender_jid(msg.sender_jid.clone())
            .sender_name(msg.sender_name.clone())
            .content(if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            })
            .outgoing(msg.outgoing)
            .status(i64::from(msg.status as u8))
            .timestamp(msg.timestamp.as_second())
            .media_type(msg.media.as_ref().map(|m| format!("{:?}", m.r#type)))
            .media_path(self.save_media_file(chat_jid, msg))
            .media_mime(msg.media.as_ref().map(|m| m.mime_type.clone()))
            .exec(&mut db)
            .await?;

        if let Some(mut chat) = ChatEntity::filter_by_jid(chat_jid)
            .first()
            .exec(&mut db)
            .await?
        {
            chat.update()
                .last_message_time(Some(msg.timestamp.as_second()))
                .exec(&mut db)
                .await?;
        }

        Ok(())
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
            && MessageEntity::filter_by_server_id(Some(msg.server_id.clone()))
                .first()
                .exec(&mut db)
                .await?
                .is_some()
        {
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
            .sender_name(msg.sender_name.clone())
            .content(if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            })
            .outgoing(msg.outgoing)
            .status(i64::from(msg.status as u8))
            .timestamp(msg.timestamp.as_second())
            .media_type(msg.media.as_ref().map(|m| format!("{:?}", m.r#type)))
            .media_path(self.save_media_file(chat_jid, msg))
            .media_mime(msg.media.as_ref().map(|m| m.mime_type.clone()))
            .exec(&mut db)
            .await?;

        if let Some(mut chat) = ChatEntity::filter_by_jid(chat_jid)
            .first()
            .exec(&mut db)
            .await?
        {
            chat.update()
                .last_message_time(Some(msg.timestamp.as_second()))
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
            .map(|entity| self.message_from_entity(entity)))
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
        .map(|entity| self.message_from_entity(entity)))
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
            .map(|entity| self.message_from_entity(entity))
            .collect::<Vec<_>>())
    }

    /// Loads messages after a timestamp, oldest first, up to `limit`.
    pub async fn load_messages_after(
        &self,
        chat_jid: &str,
        after_timestamp: i64,
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
            .map(|entity| self.message_from_entity(entity))
            .collect::<Vec<_>>())
    }

    /// Loads messages before a timestamp, newest first, up to `limit`.
    pub async fn load_messages_before(
        &self,
        chat_jid: &str,
        before_timestamp: i64,
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
            .map(|entity| self.message_from_entity(entity))
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
            .map(|entity| self.message_from_entity(entity))
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
        Contact::upsert_by_jid(contact.jid.clone())
            .name(contact.name.clone())
            .push_name(contact.push_name.clone())
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
                .map(|entity| (entity.chat_jid.clone(), self.message_from_entity(entity)))
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
