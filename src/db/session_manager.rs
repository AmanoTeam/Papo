use std::{fs, io, path::Path};

use jiff::Timestamp;
use toasty::Db;
use uuid::Uuid;

use crate::{
    DATA_DIR,
    db::{
        DbError,
        connection::{open_encrypted_db, open_main_db},
        entities::{Chat, Contact, Message, Session},
        keyring::KeyringService,
    },
};

/// Manages `WhatsApp` sessions in the central `papo.db` database.
///
/// Each session is a separate encrypted database file (`{uuid}.session`)
/// containing chat history, messages, contacts, and (after the backend
/// cutover in Step 5) the whatsapp-rust protocol state.
///
/// The encryption key for each session file is stored in the system keyring
/// via [`KeyringService`], keyed by the session UUID.
pub struct SessionManager {
    main_db: Db,
    keyring: KeyringService,
}

impl SessionManager {
    /// Creates a new session manager by opening the central `papo.db`.
    ///
    /// The database is encrypted with AES-256-GCM; the key is fetched or
    /// created via the keyring service.
    pub async fn new(keyring: KeyringService) -> Result<Self, DbError> {
        let main_db = open_main_db(&keyring).await?;
        Ok(Self { main_db, keyring })
    }

    /// Creates a new session with a fresh UUID and encrypted database file.
    ///
    /// Generates a UUID v4, creates a keyring entry for the session encryption
    /// key, initializes the `{uuid}.session` database with the chat/message/
    /// contact schema, and inserts a row into the central `papo.db`.
    pub async fn create_session(&mut self) -> Result<Session, DbError> {
        let uuid = Uuid::new_v4().to_string();
        let path = DATA_DIR.join(format!("{uuid}.session"));
        let key = self.keyring.get_or_create_session_key(&uuid).await?;
        open_encrypted_db(&path, &key, || toasty::models!(Chat, Message, Contact)).await?;

        let timestamp = Timestamp::now().as_second();
        Session::create()
            .uuid(uuid.clone())
            .path(path.to_string_lossy().to_string())
            .created_at(timestamp)
            .last_active(timestamp)
            .exec(&mut self.main_db)
            .await?;
        let session = Session::filter_by_uuid(uuid)
            .first()
            .exec(&mut self.main_db)
            .await?
            .ok_or(DbError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "session not found after insert",
            )))?;

        Ok(session)
    }

    /// Lists all sessions in the central database.
    ///
    /// Returns sessions ordered by last activity (most recent first).
    pub fn list_sessions() -> Vec<Session> {
        // TODO: implement with toasty `Session::all()` or equivalent query once
        // the toasty list-all API is confirmed.
        Vec::new()
    }

    /// Updates the `last_active` timestamp of the given session to now.
    ///
    /// Used during auto-login to mark the session the user selected.
    pub async fn mark_active(&mut self, uuid: &str) -> Result<(), DbError> {
        let timestamp = Timestamp::now().as_second();
        let mut session = Session::filter_by_uuid(uuid)
            .first()
            .exec(&mut self.main_db)
            .await?
            .ok_or(DbError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "session not found",
            )))?;

        session.last_active = timestamp;
        session.update().exec(&mut self.main_db).await?;

        Ok(())
    }

    /// Permanently deletes a session and all associated data.
    ///
    /// Removes the session row from `papo.db`, deletes the encrypted
    /// `{uuid}.session` database file (including WAL/SHM sidecars),
    /// removes the encryption key from the system keyring, and deletes
    /// the session's media directory.
    pub async fn delete_session(&mut self, uuid: &str) -> Result<(), DbError> {
        let session = Session::filter_by_uuid(uuid)
            .first()
            .exec(&mut self.main_db)
            .await?
            .ok_or(DbError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "session not found",
            )))?;

        let path = session.path.clone();
        session.delete().exec(&mut self.main_db).await?;
        self.keyring.delete_session_key(uuid).await.ok();

        if !path.is_empty() {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(format!("{path}-wal"));
            let _ = fs::remove_file(format!("{path}-shm"));
        }

        let media_dir = DATA_DIR.join("media").join(uuid);
        let _ = fs::remove_dir_all(&media_dir);

        Ok(())
    }

    /// Returns the most recently active session whose database file exists.
    ///
    /// Used at startup for auto-login: if a valid session is found,
    /// the app skips the login screen and opens that session directly.
    /// Sessions whose database file is missing or corrupt are skipped.
    pub fn last_valid_session() -> Option<Session> {
        let sessions = Self::list_sessions();
        let mut best = None::<Session>;

        for session in sessions {
            if best
                .as_ref()
                .is_none_or(|b| session.last_active > b.last_active)
            {
                let path = session.path.clone();
                if !path.is_empty() && Path::new(&path).exists() {
                    best = Some(session);
                }
            }
        }

        best
    }

    /// Returns a reference to the central database handle.
    pub fn main_db(&self) -> &Db {
        &self.main_db
    }

    /// Returns a reference to the keyring service.
    pub fn keyring(&self) -> &KeyringService {
        &self.keyring
    }
}
