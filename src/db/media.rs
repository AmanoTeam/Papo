use std::{fs, io, path::PathBuf};

use crate::DATA_DIR;

/// Media file storage for message attachments.
///
/// Media files are stored on disk under `DATA_DIR/media/{session_uuid}/{chat_jid}/`.
/// This allows lazy loading of media content. Files are named
/// by the message's `local_id` with an extension derived from
/// the media type.
pub struct MediaStorage {
    session_uuid: String,
}

impl MediaStorage {
    /// Creates a new media storage helper for the given session.
    pub fn new(session_uuid: &str) -> Self {
        Self {
            session_uuid: session_uuid.to_string(),
        }
    }

    /// Returns the base media directory for this session.
    pub fn base_dir(&self) -> PathBuf {
        DATA_DIR.join("media").join(&self.session_uuid)
    }

    /// Returns the media directory for a specific chat.
    fn chat_dir(&self, chat_jid: &str) -> PathBuf {
        self.base_dir().join(sanitize_component(chat_jid))
    }

    /// Returns the full path for a media file.
    ///
    /// The path is `DATA_DIR/media/{session}/{chat}/{message_id}.{ext}`.
    /// All path components are sanitized to prevent directory traversal.
    pub fn media_path(&self, chat_jid: &str, message_id: &str, extension: &str) -> PathBuf {
        self.chat_dir(chat_jid)
            .join(format!("{}.{}", sanitize_component(message_id), extension))
    }

    /// Saves media bytes to disk and returns the relative path.
    ///
    /// The returned path is relative to `DATA_DIR` so it can be stored
    /// in the database and resolved later via [`resolve_path`].
    pub fn save_media(
        &self,
        chat_jid: &str,
        message_id: &str,
        extension: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        let path = self.media_path(chat_jid, message_id, extension);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, data)?;

        let relative = path
            .strip_prefix(&*DATA_DIR)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        Ok(relative)
    }

    /// Loads media bytes from a relative path stored in the database.
    pub fn load_media(relative_path: &str) -> Result<Vec<u8>, io::Error> {
        let path = Self::resolve_path(relative_path);
        fs::read(&path)
    }

    /// Resolves a relative path stored in the database to a full filesystem path.
    pub fn resolve_path(relative_path: &str) -> PathBuf {
        DATA_DIR.join(relative_path)
    }

    /// Deletes a media file by its relative path.
    pub fn delete_media(relative_path: &str) {
        let path = Self::resolve_path(relative_path);
        let _ = fs::remove_file(&path);
    }

    /// Deletes all media for a specific chat.
    pub fn delete_chat_media(&self, chat_jid: &str) {
        let dir = self.chat_dir(chat_jid);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Deletes all media for this session.
    pub fn delete_session_media(&self) {
        let dir = self.base_dir();
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Sanitizes a string for use as a filesystem path component.
///
/// Replaces path separators and special characters with underscores
/// to prevent directory traversal.
fn sanitize_component(component: &str) -> String {
    component
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn media_save_load_delete() {
        let session = Uuid::new_v4().to_string();
        let storage = MediaStorage::new(&session);
        let chat = "test@s.whatsapp.net";
        let msg_id = Uuid::new_v4().to_string();
        let data = b"fake image bytes";

        let relative = storage.save_media(chat, &msg_id, "jpg", data).unwrap();
        assert!(!relative.is_empty());

        let loaded = MediaStorage::load_media(&relative).unwrap();
        assert_eq!(loaded, data);

        MediaStorage::delete_media(&relative);
        assert!(MediaStorage::load_media(&relative).is_err());

        storage.delete_session_media();
    }

    #[test]
    fn path_sanitization() {
        let session = Uuid::new_v4().to_string();
        let storage = MediaStorage::new(&session);
        let path = storage.media_path("user/..%2Fevil", "msg@id", "jpg");
        let path_str = path.to_string_lossy();
        let chat_component = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        assert!(!chat_component.contains(".."));
        assert!(!chat_component.contains('/'));
    }
}
