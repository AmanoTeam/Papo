use std::{
    cell::RefCell,
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use moka::future::Cache;
use tokio::sync::OnceCell;
use wacore::client::context::GroupInfo;
use whatsapp_rust::ContactInfo;

use crate::state::{Chat, ChatMessage};

/// Runtime cache for WhatsApp data fetched from network.
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
                    .time_to_live(Duration::from_secs(3600))
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
                    .time_to_live(Duration::from_secs(3600))
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
                    .time_to_live(Duration::from_secs(3600))
                    .max_capacity(2_000)
                    .build()
            })
            .await
    }
}

/// Cache for chat list data.
#[derive(Clone)]
pub struct ChatListCache {
    pub count: usize,
    pub chats: Arc<[Chat]>,
    pub last_updated: Instant,
}

/// Cache for messages in a specific chat.
#[derive(Clone)]
pub struct MessageListCache {
    pub count: usize,
    pub messages: Arc<[ChatMessage]>,
    pub max_media_size: f32,
}

/// UI render cache with interior mutability.
/// This avoids recomputing expensive UI data on every render.
pub struct RenderCache {
    /// Chat list cache, None means needs recompute.
    chat_list: RefCell<Option<ChatListCache>>,
    /// Message list cache per chat JID.
    message_lists: RefCell<HashMap<String, MessageListCache>>,
}

impl RenderCache {
    /// Create a new empty render cache.
    pub fn new() -> Self {
        Self {
            chat_list: RefCell::new(None),
            message_lists: RefCell::new(HashMap::new()),
        }
    }

    /// Get or compute chat list cache.
    /// Uses count comparison for cheap invalidation check.
    pub fn get_chat_list(&self, chats: &[Chat]) -> Arc<[Chat]> {
        let mut cache = self.chat_list.borrow_mut();

        // Check if cache is still valid (compare count).
        if let Some(ref cached) = *cache
            && cached.count == chats.len()
        {
            return cached.chats.clone();
        }

        // Cache miss - recompute.
        let chats_arc = chats.iter().cloned().collect::<Arc<[Chat]>>();
        *cache = Some(ChatListCache {
            count: chats_arc.len(),
            chats: chats_arc.clone(),
            last_updated: std::time::Instant::now(),
        });

        chats_arc
    }

    /// Invalidate chat list cache (call when chats change).
    pub fn invalidate_chat_list(&self) {
        *self.chat_list.borrow_mut() = None;
    }

    /// Get or compute message list cache for a chat.
    pub fn get_message_list(
        &self,
        chat_jid: &str,
        messages: &[ChatMessage],
        max_media_size: f32,
    ) -> Arc<[ChatMessage]> {
        let mut caches = self.message_lists.borrow_mut();

        // Check if cache is valid.
        if let Some(cached) = caches.get(chat_jid)
            && cached.count == messages.len()
            && cached.max_media_size == max_media_size
        {
            return cached.messages.clone();
        }

        // Cache miss - recompute.
        let messages_arc = messages.iter().cloned().collect::<Arc<[ChatMessage]>>();
        caches.insert(
            chat_jid.to_string(),
            MessageListCache {
                count: messages_arc.len(),
                messages: messages_arc.clone(),
                max_media_size,
            },
        );

        messages_arc
    }

    /// Invalidate message cache for a specific chat.
    pub fn invalidate_message_list(&self, chat_jid: &str) {
        self.message_lists.borrow_mut().remove(chat_jid);
    }

    /// Invalidate all message caches.
    pub fn invalidate_all_messages(&self) {
        self.message_lists.borrow_mut().clear();
    }
}
