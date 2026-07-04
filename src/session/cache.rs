use std::{fs, io, path::PathBuf, time::Duration};

use moka::future::Cache;
use tokio::sync::OnceCell;
use wacore::client::context::GroupInfo;
use whatsapp_rust::ContactInfo;

use crate::DATA_DIR;

/// Cache for chat avatars downloaded from `WhatsApp`.
#[derive(Clone, Debug)]
pub struct AvatarCache {
    /// Directory where avatars are stored.
    cache_dir: PathBuf,
}

impl AvatarCache {
    /// Create a new avatar cache.
    pub fn new() -> Result<Self, io::Error> {
        let cache_dir = DATA_DIR.join("avatars");
        fs::create_dir_all(&cache_dir)?;

        Ok(Self { cache_dir })
    }

    /// Get the path for a cached avatar.
    pub fn get_avatar_path(&self, jid: &str) -> PathBuf {
        // Sanitize JID for use as filename.
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
    pub fn save_avatar(&self, jid: &str, data: &[u8]) -> Result<String, io::Error> {
        let path = self.get_avatar_path(jid);
        fs::write(&path, data)?;

        Ok(path.to_string_lossy().into_owned())
    }

    /// Delete a cached avatar.
    pub fn delete_avatar(&self, jid: &str) -> Result<(), io::Error> {
        let path = self.get_avatar_path(jid);
        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    /// Clear all cached avatars.
    pub fn clear_cache(&self) -> Result<(), io::Error> {
        if self.cache_dir.exists() {
            fs::remove_dir_all(&self.cache_dir)?;
            fs::create_dir_all(&self.cache_dir)?;
        }

        Ok(())
    }
}

/// Runtime cache for `WhatsApp` data fetched from network.
/// Uses Moka for automatic TTL eviction.
pub struct RuntimeCache {
    /// Group info cache, maps JID -> group metadata.
    groups: OnceCell<Cache<String, GroupInfo>>,
    /// Device cache, maps user JID -> device.
    devices: OnceCell<Cache<String, Vec<String>>>,
    /// Contact cache, maps JID -> contact info.
    contacts: OnceCell<Cache<String, ContactInfo>>,
}

impl RuntimeCache {
    /// Create a new empty runtime cache.
    pub fn new() -> Self {
        Self {
            groups: OnceCell::new(),
            devices: OnceCell::new(),
            contacts: OnceCell::new(),
        }
    }

    /// Get or initialize group cache.
    pub async fn get_groups(&self) -> &Cache<String, GroupInfo> {
        self.groups
            .get_or_init(|| async {
                tracing::debug!("Initializing group cache...");

                Cache::builder()
                    .time_to_live(Duration::from_hours(1))
                    .max_capacity(1_000)
                    .build()
            })
            .await
    }

    /// Get or initialize device cache.
    pub async fn get_devices(&self) -> &Cache<String, Vec<String>> {
        self.devices
            .get_or_init(|| async {
                tracing::debug!("Initializing device cache...");

                Cache::builder()
                    .time_to_live(Duration::from_hours(1))
                    .max_capacity(5_000)
                    .build()
            })
            .await
    }

    /// Get or initialize contact cache.
    pub async fn get_contacts(&self) -> &Cache<String, ContactInfo> {
        self.contacts
            .get_or_init(|| async {
                tracing::debug!("Initializing contact cache...");

                Cache::builder()
                    .time_to_live(Duration::from_hours(1))
                    .max_capacity(2_000)
                    .build()
            })
            .await
    }
}
