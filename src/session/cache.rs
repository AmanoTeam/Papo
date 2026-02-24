use std::time::Duration;

use moka::future::Cache;
use tokio::sync::OnceCell;
use wacore::client::context::GroupInfo;
use whatsapp_rust::ContactInfo;

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
