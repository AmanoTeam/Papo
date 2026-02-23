use std::{collections::HashMap, path::Path, sync::Arc};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use libsql::{Builder, Cipher, Connection, EncryptionConfig};

use crate::{
    config::PAPO_DATABASE_PATH,
    state::{Chat, ChatMessage, Media, MediaType},
};

/// Papo's own database for UI state persistence.
/// Separate from whatsapp-rust's protocol database.
#[derive(Clone, Debug)]
pub struct Database {
    db: Arc<libsql::Database>,
    conn: Arc<Connection>,
}

impl Database {
    /// Create a new database.
    pub async fn new() -> Result<Self, libsql::Error> {
        let path = PAPO_DATABASE_PATH;

        // Create parent directory.
        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        let db = Arc::new(
            Builder::new_local(path)
                .encryption_config(EncryptionConfig {
                    cipher: Cipher::Aes256Cbc,
                    encryption_key: "".into(), // TODO: use a proper encryption key
                })
                .build()
                .await?,
        );
        let conn = Arc::new(db.connect()?);

        let this = Self { db, conn };
        this.init_tables().await?;

        Ok(this)
    }

    /// Initialize the database tables.
    async fn init_tables(&self) -> Result<(), libsql::Error> {
        // Chats.
        self.conn
            .execute(
                r"
            CREATE TABLE IF NOT EXISTS chats (
                jid TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                muted INTEGER DEFAULT 0,
                pinned INTEGER DEFAULT 0,
                unread_count INTEGER DEFAULT 0,
                last_message_time INTEGER,
                archived INTEGER DEFAULT 0
            )
            ",
                (),
            )
            .await?;

        // Messages.
        self.conn
            .execute(
                r"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                chat_jid TEXT NOT NULL,
                sender_jid TEXT NOT NULL,
                sender_name TEXT,
                content TEXT,
                outgoing INTEGER DEFAULT 0,
                unread INTEGER DEFAULT 1,
                timestamp INTEGER NOT NULL,
                media_type TEXT,
                media_data BLOB,
                FOREIGN KEY (chat_jid) REFERENCES chats(jid) ON DELETE CASCADE
            )
            ",
                (),
            )
            .await?;

        // Contacts.
        self.conn
            .execute(
                r"
            CREATE TABLE IF NOT EXISTS contacts (
                jid TEXT PRIMARY KEY,
                phone_number TEXT,
                name TEXT,
                push_name TEXT,
                profile_picture_url TEXT,
                is_registered INTEGER DEFAULT 0,
                last_updated INTEGER
            )
            ",
                (),
            )
            .await?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_chat ON messages(chat_jid, timestamp DESC)",
            (),
        )
        .await?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_chats_pinned ON chats(pinned DESC, last_message_time DESC)",
            (),
        ).await?;
        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_contacts_jid ON contacts(jid)",
                (),
            )
            .await?;

        Ok(())
    }
}

/// Chat operations
impl Database {
    pub async fn save_chat(&self, chat: &Chat) -> Result<(), libsql::Error> {
        let last_msg_time = chat
            .get_last_message()
            .await
            .expect("Failed to get the last message of a chat")
            .map_or(0, |m| m.timestamp.timestamp());

        self.conn
            .execute(
                r"
            INSERT INTO chats (jid, name, muted, pinned, unread_count, last_message_time, archived)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(jid) DO UPDATE SET
                name = excluded.name,
                muted = excluded.muted,
                pinned = excluded.pinned,
                unread_count = excluded.unread_count,
                last_message_time = excluded.last_message_time,
                archived = excluded.archived
            ",
                libsql::params![
                    chat.jid.clone(),
                    chat.name.clone(),
                    i32::from(chat.muted),
                    i32::from(chat.pinned),
                    chat.unread_count,
                    last_msg_time,
                    0i32 // archived
                ],
            )
            .await?;

        Ok(())
    }

    pub async fn load_chat(&self, jid: &str) -> Result<Option<Chat>, libsql::Error> {
        let mut rows = self
            .conn
            .query(
                r"
            SELECT jid, name, muted, pinned, unread_count, last_message_time
            FROM chats
            WHERE jid = ?1 AND archived = 0
            ORDER BY pinned DESC, last_message_time DESC
            LIMIT 1
            ",
                [jid],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let jid: String = row.get(0)?;

            Ok(Some(Chat {
                jid,
                name: row.get(1)?,
                muted: row.get::<i32>(2)? != 0,
                pinned: row.get::<i32>(3)? != 0,
                unread_count: row.get::<u32>(4)?,
                participants: HashMap::new(),
                last_message_time: DateTime::from_timestamp(row.get::<i64>(5)?, 0)
                    .expect("Invalid timestamp"),

                db: Arc::new(self.clone()),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn load_chats(&self) -> Result<Vec<Chat>, libsql::Error> {
        let mut rows = self
            .conn
            .query(
                r"
            SELECT jid, name, muted, pinned, unread_count, last_message_time
            FROM chats
            WHERE archived = 0
            ORDER BY pinned DESC, last_message_time DESC
            ",
                (),
            )
            .await?;

        let mut chats = Vec::new();
        while let Some(row) = rows.next().await? {
            let jid: String = row.get(0)?;

            chats.push(Chat {
                jid,
                name: row.get(1)?,
                muted: row.get::<i32>(2)? != 0,
                pinned: row.get::<i32>(3)? != 0,
                unread_count: row.get::<u32>(4)?,
                participants: HashMap::new(),
                last_message_time: DateTime::from_timestamp(row.get::<i64>(5)?, 0)
                    .expect("Invalid timestamp"),

                db: Arc::new(self.clone()),
            });
        }

        Ok(chats)
    }

    pub async fn delete_chat(&self, jid: &str) -> Result<(), libsql::Error> {
        // Cascade delete will remove messages too.
        self.conn
            .execute("DELETE FROM chats WHERE jid = ?1", [jid])
            .await?;

        Ok(())
    }

    pub async fn update_chat_unread(&self, jid: &str, count: u32) -> Result<(), libsql::Error> {
        self.conn
            .execute(
                "UPDATE chats SET unread_count = ?1 WHERE jid = ?2",
                libsql::params![count, jid],
            )
            .await?;

        Ok(())
    }
}

/// Message operations.
impl Database {
    pub async fn save_message(
        &self,
        chat_jid: &str,
        msg: &ChatMessage,
    ) -> Result<(), libsql::Error> {
        let media_type = msg.media.as_ref().map(|m| format!("{:?}", m.r#type));
        let media_data = msg.media.as_ref().map(|m| m.data.as_ref().clone());

        self.conn
            .execute(
                r"
            INSERT INTO messages (id, chat_jid, sender_jid, sender_name, content,
                                  outgoing, unread, timestamp, media_type, media_data)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                unread = excluded.unread,
                content = excluded.content
            ",
                libsql::params![
                    msg.id.clone(),
                    chat_jid,
                    msg.sender_jid.clone(),
                    msg.sender_name.clone(),
                    msg.content.clone(),
                    i32::from(msg.outgoing),
                    i32::from(msg.unread),
                    msg.timestamp.timestamp(),
                    media_type,
                    media_data
                ],
            )
            .await?;

        // Update chat's last_message_time.
        self.conn
            .execute(
                "UPDATE chats SET last_message_time = ?1 WHERE jid = ?2",
                libsql::params![msg.timestamp.timestamp(), chat_jid],
            )
            .await?;

        Ok(())
    }

    pub async fn load_message(
        &self,
        chat_jid: &str,
        msg_id: &str,
    ) -> Result<Option<ChatMessage>, libsql::Error> {
        let mut rows = self.conn.query(
            r"
            SELECT id, chat_jid, sender_jid, sender_name, content, outgoing, unread, timestamp, media_type, media_data
            FROM messages
            WHERE chat_jid = ?1 AND id = ?2
            ORDER BY timestamp DESC
            LIMIT ?2
            ",
            libsql::params![chat_jid, msg_id],
        ).await?;

        if let Some(row) = rows.next().await? {
            let media = row.get::<String>(8).map_or(None, |media_type| {
                row.get::<Vec<u8>>(9).map_or(None, |data| {
                    let media_type: MediaType = media_type.into();

                    Some(Media {
                        data: Arc::new(data),
                        r#type: media_type,
                        mime_type: media_type.guess_mime_type(),
                        ..Default::default()
                    })
                })
            });

            Ok(Some(ChatMessage {
                id: row.get(0)?,
                chat_jid: row.get(1)?,
                sender_jid: row.get(2)?,
                sender_name: row.get(3).ok(),

                media,
                unread: row.get::<i32>(6)? != 0,
                content: row.get(4)?,
                outgoing: row.get::<i32>(5)? != 0,
                timestamp: DateTime::from_timestamp(row.get::<i64>(7)?, 0).unwrap_or_else(Utc::now),
                reactions: IndexMap::new(),

                db: Arc::new(self.clone()),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn load_messages(
        &self,
        chat_jid: &str,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, libsql::Error> {
        let mut rows = self.conn.query(
            r"
            SELECT id, chat_jid, sender_jid, sender_name, content, outgoing, unread, timestamp, media_type, media_data
            FROM messages
            WHERE chat_jid = ?1
            ORDER BY timestamp DESC
            LIMIT ?2
            ",
            libsql::params![chat_jid, limit],
        ).await?;

        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            let media = row.get::<String>(8).map_or(None, |media_type| {
                row.get::<Vec<u8>>(9).map_or(None, |data| {
                    let media_type: MediaType = media_type.into();

                    Some(Media {
                        data: Arc::new(data),
                        r#type: media_type,
                        mime_type: media_type.guess_mime_type(),
                        ..Default::default()
                    })
                })
            });

            messages.push(ChatMessage {
                id: row.get(0)?,
                chat_jid: row.get(1)?,
                sender_jid: row.get(2)?,
                sender_name: row.get(3).ok(),

                media,
                unread: row.get::<i32>(6)? != 0,
                content: row.get(4)?,
                outgoing: row.get::<i32>(5)? != 0,
                timestamp: DateTime::from_timestamp(row.get::<i64>(7)?, 0).unwrap_or_else(Utc::now),
                reactions: IndexMap::new(),

                db: Arc::new(self.clone()),
            });
        }

        Ok(messages)
    }

    /// Load messages from before a specific time.
    pub async fn load_messages_before(
        &self,
        chat_jid: &str,
        before_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, libsql::Error> {
        let mut rows = self
            .conn
            .query(
                r"
            SELECT id, chat_jid, sender_jid, sender_name, content, outgoing, unread, timestamp
            FROM messages
            WHERE chat_jid = ?1 AND timestamp < ?2
            ORDER BY timestamp DESC
            LIMIT ?3
            ",
                libsql::params![chat_jid, before_timestamp, limit],
            )
            .await?;

        let mut messages = Vec::new();
        while let Some(row) = rows.next().await? {
            messages.push(ChatMessage {
                id: row.get(0)?,
                chat_jid: row.get(1)?,
                sender_jid: row.get(1)?,
                sender_name: row.get(2).ok(),

                media: None,
                unread: row.get::<i32>(6)? != 0,
                content: row.get(4)?,
                outgoing: row.get::<i32>(5)? != 0,
                timestamp: DateTime::from_timestamp(row.get::<i64>(7)?, 0).unwrap_or_else(Utc::now),
                reactions: IndexMap::new(),

                db: Arc::new(self.clone()),
            });
        }

        Ok(messages)
    }

    pub async fn mark_message_read(&self, message_id: &str) -> Result<(), libsql::Error> {
        self.conn
            .execute("UPDATE messages SET unread = 0 WHERE id = ?1", [message_id])
            .await?;

        Ok(())
    }

    /// Mark all messages from a chat as read.
    pub async fn mark_chat_read(&self, chat_jid: &str) -> Result<(), libsql::Error> {
        self.conn
            .execute(
                "UPDATE messages SET unread = 0 WHERE chat_jid = ?1",
                [chat_jid],
            )
            .await?;

        self.conn
            .execute(
                "UPDATE chats SET unread_count = 0 WHERE jid = ?1",
                [chat_jid],
            )
            .await?;

        Ok(())
    }

    pub async fn delete_message(&self, message_id: &str) -> Result<(), libsql::Error> {
        self.conn
            .execute("DELETE FROM messages WHERE id = ?1", [message_id])
            .await?;

        Ok(())
    }

    pub async fn get_unread_count(&self, chat_jid: &str) -> Result<usize, libsql::Error> {
        let mut rows = self
            .conn
            .query(
                "SELECT COUNT(*) FROM messages WHERE chat_jid = ?1 AND unread = 1",
                [chat_jid],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(usize::try_from(row.get::<u64>(0)?).unwrap_or(0))
        } else {
            Ok(0)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Contact {
    pub jid: String,
    pub name: Option<String>,
    pub push_name: Option<String>,
    pub phone_number: Option<String>,
    pub is_registered: bool,
}

/// Contact operations.
impl Database {
    pub async fn save_contact(&self, contact: &Contact) -> Result<(), libsql::Error> {
        self.conn
            .execute(
                r"
            INSERT INTO contacts (jid, phone_number, name, push_name, is_registered, last_updated)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(jid) DO UPDATE SET
                phone_number = excluded.phone_number,
                name = excluded.name,
                push_name = excluded.push_name,
                is_registered = excluded.is_registered,
                last_updated = excluded.last_updated
            ",
                libsql::params![
                    contact.jid.clone(),
                    contact.phone_number.clone(),
                    contact.name.clone(),
                    contact.push_name.clone(),
                    i32::from(contact.is_registered),
                    Utc::now().timestamp()
                ],
            )
            .await?;

        Ok(())
    }

    pub async fn get_contact(&self, jid: &str) -> Result<Option<Contact>, libsql::Error> {
        let mut rows = self.conn.query(
            "SELECT jid, phone_number, name, push_name, is_registered FROM contacts WHERE jid = ?1",
            [jid],
        ).await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(Contact {
                jid: row.get(0)?,
                name: row.get(2).ok(),
                push_name: row.get(3).ok(),
                phone_number: row.get(1).ok(),
                is_registered: row.get::<i32>(4)? != 0,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_all_contacts(&self) -> Result<Vec<Contact>, libsql::Error> {
        let mut rows = self.conn.query(
            "SELECT jid, phone_number, name, push_name, is_registered FROM contacts ORDER BY name",
            (),
        ).await?;

        let mut contacts = Vec::new();
        while let Some(row) = rows.next().await? {
            contacts.push(Contact {
                jid: row.get(0)?,
                name: row.get(2).ok(),
                push_name: row.get(3).ok(),
                phone_number: row.get(1).ok(),
                is_registered: row.get::<i32>(4)? != 0,
            });
        }

        Ok(contacts)
    }
}

/// Search operations.
impl Database {
    pub async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>, libsql::Error> {
        let search_pattern = format!("%{query}%");

        let mut rows = self
            .conn
            .query(
                r"
            SELECT jid, phone_number, name, push_name, is_registered
            FROM contacts
            WHERE name LIKE ?1 OR push_name LIKE ?1 OR jid LIKE ?1
            ORDER BY name
            ",
                [search_pattern],
            )
            .await?;

        let mut contacts = Vec::new();
        while let Some(row) = rows.next().await? {
            contacts.push(Contact {
                jid: row.get(0)?,
                name: row.get(2).ok(),
                push_name: row.get(3).ok(),
                phone_number: row.get(1).ok(),
                is_registered: row.get::<i32>(4)? != 0,
            });
        }

        Ok(contacts)
    }

    pub async fn search_messages(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<(String, ChatMessage)>, libsql::Error> {
        let search_pattern = format!("%{query}%");

        let mut rows = self
            .conn
            .query(
                r"
            SELECT id, chat_jid, sender_jid, sender_name, content, outgoing, unread, timestamp
            FROM messages
            WHERE content LIKE ?1
            ORDER BY timestamp DESC
            LIMIT ?2
            ",
                libsql::params![search_pattern, limit],
            )
            .await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let chat_jid: String = row.get(1)?;
            let message = ChatMessage {
                id: row.get(0)?,
                chat_jid: chat_jid.clone(),
                sender_jid: row.get(2)?,
                sender_name: row.get(3).ok(),

                media: None,
                unread: row.get::<i32>(6)? != 0,
                content: row.get(4)?,
                outgoing: row.get::<i32>(5)? != 0,
                timestamp: DateTime::from_timestamp(row.get::<i64>(7)?, 0).unwrap_or_else(Utc::now),
                reactions: IndexMap::new(),

                db: Arc::new(self.clone()),
            };
            results.push((chat_jid, message));
        }

        Ok(results)
    }
}
