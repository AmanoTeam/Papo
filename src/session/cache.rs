use std::{fs, io, path::PathBuf, time::Duration};

use crate::CACHE_DIR;

/// Cache for chat avatars downloaded from `WhatsApp`.
#[derive(Clone, Debug)]
pub struct AvatarCache {
    /// Directory where avatars are stored.
    cache_dir: PathBuf,
}

impl AvatarCache {
    const TTL_TIME: Duration = Duration::new(7 * 24 * 60 * 60, 0);

    pub fn new() -> Result<Self, io::Error> {
        let cache_dir = CACHE_DIR.join("avatars");
        fs::create_dir_all(&cache_dir)?;

        Ok(Self { cache_dir })
    }

    pub fn is_cached(&self, jid: &str) -> bool {
        self.get_avatar_path(jid).exists()
    }

    pub fn is_stale(&self, jid: &str) -> bool {
        fs::metadata(self.get_avatar_path(jid)).map_or(true, |m| {
            m.modified()
                .map_or(true, |t| t.elapsed().is_ok_and(|e| e > Self::TTL_TIME))
        })
    }

    pub fn get_avatar_path(&self, jid: &str) -> PathBuf {
        // Sanitize JID for use as filename.
        let safe_jid = jid.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");

        self.cache_dir.join(format!("{safe_jid}.jpg"))
    }

    /// Get the cached avatar path if it exists.
    pub fn get_cached_path(&self, jid: &str) -> Option<String> {
        let path = self.get_avatar_path(jid);
        path.exists().then(|| path.to_string_lossy().into_owned())
    }

    pub fn save_avatar(&self, jid: &str, data: &[u8]) -> Result<String, io::Error> {
        let path = self.get_avatar_path(jid);
        fs::write(&path, data)?;

        Ok(path.to_string_lossy().into_owned())
    }

    pub fn delete_avatar(&self, jid: &str) -> Result<(), io::Error> {
        let path = self.get_avatar_path(jid);
        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    pub fn clear_cache(&self) -> Result<(), io::Error> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
            fs::create_dir_all(&self.cache_dir)?;
        }

        Ok(())
    }
}
