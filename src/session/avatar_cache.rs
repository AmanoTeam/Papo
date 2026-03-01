use std::{fs, path::PathBuf};

use crate::DATA_DIR;

/// Cache for chat avatars downloaded from `WhatsApp`.
#[derive(Clone, Debug)]
pub struct AvatarCache {
    /// Directory where avatars are stored.
    cache_dir: PathBuf,
}

impl AvatarCache {
    /// Create a new avatar cache.
    pub fn new() -> Result<Self, std::io::Error> {
        let cache_dir = DATA_DIR.join("avatars");
        fs::create_dir_all(&cache_dir)?;

        Ok(Self { cache_dir })
    }

    /// Get the path for a cached avatar.
    pub fn get_avatar_path(&self, jid: &str) -> PathBuf {
        // Sanitize JID for use as filename
        let safe_jid = jid.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.cache_dir.join(format!("{safe_jid}.jpg"))
    }

    /// Check if an avatar is cached.
    pub fn is_cached(&self, jid: &str) -> bool {
        self.get_avatar_path(jid).exists()
    }

    /// Get the cached avatar path if it exists.
    pub fn get_cached_path(&self, jid: &str) -> Option<String> {
        let path = self.get_avatar_path(jid);
        path.exists().then(|| path.to_string_lossy().into_owned())
    }

    /// Save avatar bytes to cache.
    pub fn save_avatar(&self, jid: &str, data: &[u8]) -> Result<String, std::io::Error> {
        let path = self.get_avatar_path(jid);
        fs::write(&path, data)?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Delete a cached avatar.
    pub fn delete_avatar(&self, jid: &str) -> Result<(), std::io::Error> {
        let path = self.get_avatar_path(jid);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Clear all cached avatars.
    pub fn clear_cache(&self) -> Result<(), std::io::Error> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
            fs::create_dir_all(&self.cache_dir)?;
        }
        Ok(())
    }
}
