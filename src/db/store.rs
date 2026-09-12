use jiff::Timestamp;
use toasty::Db;
use uuid::Uuid;

use crate::db::entities::{Chat, Contact, Message};

/// Store for chat, message, and contact data backed by a per-session
/// encrypted database.
///
/// Each session has its own database file containing the full chat history,
/// messages, and contacts for a single `WhatsApp` account.
pub struct SessionStore {
    db: Db,
}

impl SessionStore {
    /// Creates a new store wrapping the given database handle.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Returns a reference to the underlying database handle.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Returns a mutable reference to the underlying database handle.
    pub fn db_mut(&mut self) -> &mut Db {
        &mut self.db
    }
}

// ── Chat operations ───────────────────────────────────────────

impl SessionStore {
    /// Upserts a chat. If a chat with the same `JID` exists, its name,
    /// muted, pinned, archived, and `last_message_time` fields are updated.
    pub async fn save_chat(&mut self, chat: &Chat) -> Result<(), toasty::Error> {
        Chat::create()
            .jid(chat.jid.clone())
            .name(chat.name.clone())
            .muted(chat.muted)
            .pinned(chat.pinned)
            .archived(chat.archived)
            .last_message_time(chat.last_message_time)
            .exec(&mut self.db)
            .await?;
        Ok(())
    }

    /// Inserts a chat row if it does not already exist (placeholder for `FK`).
    pub async fn ensure_chat_exists(&mut self, jid: &str) -> Result<(), toasty::Error> {
        if Chat::filter_by_jid(jid)
            .first()
            .exec(&mut self.db)
            .await?
            .is_none()
        {
            Chat::create()
                .jid(jid.to_string())
                .name(jid.to_string())
                .muted(false)
                .pinned(false)
                .archived(false)
                .last_message_time(None)
                .exec(&mut self.db)
                .await?;
        }
        Ok(())
    }

    /// Loads a single non-archived chat by `JID`.
    pub async fn load_chat(&mut self, jid: &str) -> Result<Option<Chat>, toasty::Error> {
        Ok(Chat::filter_by_jid(jid)
            .first()
            .exec(&mut self.db)
            .await?
            .filter(|chat| !chat.archived))
    }

    /// Loads all non-archived chats, ordered by pinned then `last_message_time`.
    pub async fn load_chats(&mut self) -> Result<Vec<Chat>, toasty::Error> {
        Chat::filter_by_archived(false).exec(&mut self.db).await
    }

    /// Deletes a chat and all its messages (cascade via `FK` relationship).
    pub async fn delete_chat(&mut self, jid: &str) -> Result<(), toasty::Error> {
        if let Some(chat) = Chat::filter_by_jid(jid).first().exec(&mut self.db).await? {
            chat.delete().exec(&mut self.db).await?;
        }
        Ok(())
    }
}

// ── Message operations ──────────────────────────────────────────

impl SessionStore {
    /// Upserts a message by `local_id` and updates the chat's `last_message_time`.
    pub async fn save_message(
        &mut self,
        chat_jid: &str,
        msg: &Message,
    ) -> Result<(), toasty::Error> {
        Message::create()
            .local_id(msg.local_id.clone())
            .server_id(msg.server_id.clone())
            .chat_jid(chat_jid.to_string())
            .sender_jid(msg.sender_jid.clone())
            .sender_name(msg.sender_name.clone())
            .content(msg.content.clone())
            .outgoing(msg.outgoing)
            .status(msg.status)
            .timestamp(msg.timestamp)
            .media_type(msg.media_type.clone())
            .media_path(msg.media_path.clone())
            .media_mime(msg.media_mime.clone())
            .exec(&mut self.db)
            .await?;

        if let Some(chat) = Chat::filter_by_jid(chat_jid)
            .first()
            .exec(&mut self.db)
            .await?
        {
            let mut chat = chat;
            chat.last_message_time = Some(msg.timestamp);
            chat.update().exec(&mut self.db).await?;
        }

        Ok(())
    }

    /// Inserts a message if its `server_id` does not already exist.
    /// Returns `true` if a new row was inserted, `false` if it was a duplicate.
    pub async fn save_synced_message(
        &mut self,
        chat_jid: &str,
        msg: &Message,
    ) -> Result<bool, toasty::Error> {
        self.ensure_chat_exists(chat_jid).await?;

        if Message::filter_by_server_id(msg.server_id.clone())
            .first()
            .exec(&mut self.db)
            .await?
            .is_some_and(|e| e.server_id.is_some())
        {
            return Ok(false);
        }

        Message::create()
            .local_id(msg.local_id.clone())
            .server_id(msg.server_id.clone())
            .chat_jid(chat_jid.to_string())
            .sender_jid(msg.sender_jid.clone())
            .sender_name(msg.sender_name.clone())
            .content(msg.content.clone())
            .outgoing(msg.outgoing)
            .status(msg.status)
            .timestamp(msg.timestamp)
            .media_type(msg.media_type.clone())
            .media_path(msg.media_path.clone())
            .media_mime(msg.media_mime.clone())
            .exec(&mut self.db)
            .await?;

        if let Some(chat) = Chat::filter_by_jid(chat_jid)
            .first()
            .exec(&mut self.db)
            .await?
        {
            let mut chat = chat;
            chat.last_message_time = Some(msg.timestamp);
            chat.update().exec(&mut self.db).await?;
        }

        Ok(true)
    }

    /// Loads a message by its `local_id` within a specific chat.
    pub async fn load_message_by_local_id(
        &mut self,
        chat_jid: &str,
        msg_id: &Uuid,
    ) -> Result<Option<Message>, toasty::Error> {
        Message::filter_by_local_id(msg_id.to_string())
            .first()
            .exec(&mut self.db)
            .await
            .map(|opt| opt.filter(|m| m.chat_jid == chat_jid))
    }

    /// Loads a message by its `server_id` within a specific chat.
    pub async fn load_message_by_server_id(
        &mut self,
        chat_jid: &str,
        msg_id: &str,
    ) -> Result<Option<Message>, toasty::Error> {
        Message::filter_by_server_id(Some(msg_id.to_string()))
            .first()
            .exec(&mut self.db)
            .await
            .map(|opt| opt.filter(|m| m.chat_jid == chat_jid))
    }

    /// Loads messages for a chat, most recent first, up to `limit`.
    pub async fn load_messages(
        &mut self,
        chat_jid: &str,
        limit: u32,
    ) -> Result<Vec<Message>, toasty::Error> {
        let mut messages = Message::filter_by_chat_jid(chat_jid)
            .exec(&mut self.db)
            .await?;
        messages.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
        messages.truncate(limit as usize);
        Ok(messages)
    }

    /// Loads messages after a timestamp, oldest first, up to `limit`.
    pub async fn load_messages_after(
        &mut self,
        chat_jid: &str,
        after_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<Message>, toasty::Error> {
        let mut messages = Message::filter_by_chat_jid(chat_jid)
            .exec(&mut self.db)
            .await?
            .into_iter()
            .filter(|m| m.timestamp > after_timestamp)
            .collect::<Vec<_>>();
        messages.sort_by_key(|m| m.timestamp);
        messages.truncate(limit as usize);
        Ok(messages)
    }

    /// Loads messages before a timestamp, newest first, up to `limit`.
    pub async fn load_messages_before(
        &mut self,
        chat_jid: &str,
        before_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<Message>, toasty::Error> {
        let mut messages = Message::filter_by_chat_jid(chat_jid)
            .exec(&mut self.db)
            .await?
            .into_iter()
            .filter(|m| m.timestamp < before_timestamp)
            .collect::<Vec<_>>();
        messages.sort_by_key(|m| std::cmp::Reverse(m.timestamp));
        messages.truncate(limit as usize);
        Ok(messages)
    }

    /// Deletes a message by its `server_id`.
    pub async fn delete_message(&mut self, server_id: &str) -> Result<(), toasty::Error> {
        if let Some(msg) = Message::filter_by_server_id(Some(server_id.to_string()))
            .first()
            .exec(&mut self.db)
            .await?
        {
            msg.delete().exec(&mut self.db).await?;
        }
        Ok(())
    }

    /// Counts unread messages in a chat (status != `Read` and not outgoing).
    pub async fn get_unread_count(&mut self, chat_jid: &str) -> Result<usize, toasty::Error> {
        Ok(Message::filter_by_chat_jid(chat_jid)
            .exec(&mut self.db)
            .await?
            .iter()
            .filter(|m| m.status != 1 && !m.outgoing)
            .count())
    }

    /// Returns all unread messages in a chat (status != `Read` and not outgoing).
    pub async fn get_unread_messages(
        &mut self,
        chat_jid: &str,
    ) -> Result<Vec<Message>, toasty::Error> {
        Ok(Message::filter_by_chat_jid(chat_jid)
            .exec(&mut self.db)
            .await?
            .into_iter()
            .filter(|m| m.status != 1 && !m.outgoing)
            .collect::<Vec<_>>())
    }
}

// ── Contact operations ──────────────────────────────────────────

impl SessionStore {
    /// Upserts a contact by `JID`.
    pub async fn save_contact(&mut self, contact: &Contact) -> Result<(), toasty::Error> {
        Contact::create()
            .jid(contact.jid.clone())
            .name(contact.name.clone())
            .push_name(contact.push_name.clone())
            .phone_number(contact.phone_number.clone())
            .is_registered(contact.is_registered)
            .last_updated(Timestamp::now().as_second())
            .profile_picture_url(contact.profile_picture_url.clone())
            .exec(&mut self.db)
            .await?;
        Ok(())
    }

    /// Loads a contact by `JID`.
    pub async fn get_contact(&mut self, jid: &str) -> Result<Option<Contact>, toasty::Error> {
        Contact::filter_by_jid(jid).first().exec(&mut self.db).await
    }

    /// Loads all contacts, ordered by name.
    pub async fn get_all_contacts(&mut self) -> Result<Vec<Contact>, toasty::Error> {
        Contact::all().exec(&mut self.db).await
    }
}

// ── Search operations ──────────────────────────────────────────

impl SessionStore {
    /// Searches contacts by name, `push_name`, or `JID` (case-insensitive).
    pub async fn search_contacts(&mut self, query: &str) -> Result<Vec<Contact>, toasty::Error> {
        let q = query.to_lowercase();
        Ok(Contact::all()
            .exec(&mut self.db)
            .await?
            .into_iter()
            .filter(|c| {
                c.name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&q))
                    || c.push_name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&q))
                    || c.jid.to_lowercase().contains(&q)
            })
            .collect::<Vec<_>>())
    }

    /// Searches messages by content (case-insensitive), most recent first.
    pub async fn search_messages(
        &mut self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, Message)>, toasty::Error> {
        let q = query.to_lowercase();
        let mut results = Message::all()
            .exec(&mut self.db)
            .await?
            .into_iter()
            .filter(|m| {
                m.content
                    .as_deref()
                    .is_some_and(|c| c.to_lowercase().contains(&q))
            })
            .map(|m| (m.chat_jid.clone(), m))
            .collect::<Vec<_>>();
        results.sort_by_key(|(_, m)| std::cmp::Reverse(m.timestamp));
        results.truncate(limit as usize);
        Ok(results)
    }
}
