use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use adw::prelude::*;
use chrono::{DateTime, Utc};
use relm4::prelude::*;
use tokio::sync::Mutex;
use uuid::Uuid;
use wacore::{
    net::HttpRequest,
    pair_code::{PairCodeOptions, PlatformId},
    types::{
        events::{Event, LazyConversation},
        message::MessageInfo,
        presence::ReceiptType,
    },
};
use waproto::whatsapp::{
    Message,
    device_props::{AppVersion, PlatformType},
};
use whatsapp_rust::{Jid, TokioRuntime, bot::Bot, store::SqliteStore};
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;

use crate::{
    DATA_DIR, i18n, i18n_f,
    session::{AvatarCache, RuntimeCache},
    state::ChatMessage,
};

/// Shared client handle for accessing the `WhatsApp` client.
pub type ClientHandle = Arc<Mutex<Option<Arc<whatsapp_rust::Client>>>>;

/// `WhatsApp` client wrapper that manages the connection and provides
/// a clean interface for UI operations.
#[derive(Clone)]
pub struct Client {
    /// Client connection state.
    pub state: ClientState,
    /// Shared client reference.
    handle: ClientHandle,
    /// System OS type.
    os_type: String,

    /// Avatar cache for downloading and storing profile pictures.
    avatar_cache: Arc<Mutex<Option<AvatarCache>>>,
    /// Runtime cache shared with Application.
    runtime_cache: Arc<RuntimeCache>,
}

/// Current state of the client connection.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientState {
    /// Client is loading.
    Loading,
    /// Client is connected and authenticated.
    Connected,
    /// Client is logged out.
    LoggedOut,
    /// Connection in progress.
    Connecting,
    /// Client is disconnected.
    Disconnected,

    /// Pairing in progress.
    Pairing {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Syncing in progress.
    Syncing,

    /// Error state.
    Error(String),
}

impl ClientState {
    /// Checks if the client is paired.
    pub fn is_paired(&self) -> bool {
        matches!(self, Self::Connected | Self::Syncing)
    }
}

#[derive(Debug)]
pub enum ClientInput {
    /// Start the client connection.
    Start,
    /// Stop the client connection.
    Stop,
    /// Restart the client connection.
    Restart,

    /// Pair with a phone number.
    PairWithPhoneNumber { phone_number: String },

    /// Start a new call.
    StartCall { jid: String, is_video: bool },
    /// Accept an incoming call.
    AcceptCall { call_id: String },
    /// Decline an incoming call.
    DeclineCall { call_id: String },

    /// Send typing indicator.
    SendTyping { jid: String },
    /// Stop typing indicator.
    StopTyping { jid: String },

    /// Mark messages as read.
    MarkRead {
        chat_jid: String,
        sender_jid: Option<String>,
        message_ids: Vec<String>,
    },
    /// Send a message.
    SendMessage { message: Box<ChatMessage> },
    /// Fetch avatar for a chat.
    FetchAvatar {
        /// Chat JID.
        jid: String,
    },
}

#[derive(Debug)]
pub enum ClientOutput {
    /// Client is loading.
    Loading,
    /// Client has been successfully connected and authenticated.
    Connected {
        jid: Option<String>,
        push_name: String,
    },
    /// Client has been logged out.
    LoggedOut,
    /// Client is connecting.
    Connecting,
    /// Client has been disconnected.
    Disconnected,

    /// Self push name updated.
    SelfPushNameUpdated { push_name: String },

    /// 8-character pairing code or qr code received.
    PairCode {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Client has paired successfully.
    PairSuccess,

    /// Syncing in progress.
    Syncing,

    /// Incoming call offer.
    CallOffer {
        call_id: String,
        from_jid: String,
        is_video: bool,
    },
    /// Call ended.
    CallEnded { call_id: String },

    /// Message receipt updated.
    ReceiptUpdate {
        chat_jid: String,
        message_ids: Vec<String>,
        receipt_type: ReceiptType,
    },
    /// User presence updated.
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<DateTime<Utc>>,
    },

    /// Message was sent successfully.
    MessageSent { chat_jid: String, msg_id: Uuid },
    /// Message failed to send.
    MessageFailed { chat_jid: String, msg_id: Uuid },
    /// New message received.
    MessageReceived {
        info: Box<MessageInfo>,
        message: Box<Message>,
    },

    /// Chat synced from history (`JoinedGroup` event).
    ChatSynced {
        /// Chat JID.
        jid: String,
        /// Display name.
        name: Option<String>,
        /// Whether chat is pinned.
        pinned: bool,
        /// Whether chat is archived.
        archived: bool,
        /// Unread message count.
        unread_count: Option<u32>,
        /// Group participants (for groups).
        participants: Vec<(String, Option<String>)>,
        /// Mute end time (if muted).
        mute_end_time: Option<u64>,
        /// Last message timestamp.
        last_message_time: Option<u64>,
    },
    /// Messages synced from history for a chat.
    MessagesSynced {
        /// Chat JID.
        chat_jid: String,
        /// Synced messages.
        messages: Vec<SyncedMessage>,
    },

    /// Chat property updated (pin, mute, archive).
    ChatPropertyUpdate {
        /// Chat JID.
        jid: String,
        /// Whether the chat is pinned.
        pinned: Option<bool>,
        /// Whether the chat is muted.
        muted: Option<bool>,
        /// Whether the chat is archived.
        archived: Option<bool>,
    },

    /// History sync completed.
    HistorySyncCompleted,
    /// Offline sync completed.
    OfflineSyncCompleted,

    /// Avatar updated for a chat.
    AvatarUpdate {
        /// Chat JID.
        jid: String,
        /// Path to the cached avatar image.
        path: String,
    },
    /// Contact updated (from sync or individual update).
    ContactUpdate {
        /// Contact JID.
        jid: String,
        /// Full name from address book.
        name: Option<String>,
        /// Push name (first name).
        push_name: Option<String>,
        /// Phone number (from JID user part).
        phone_number: String,
    },

    /// Error occurred.
    Error { message: String },
}

/// A message synced from history.
#[derive(Debug, Clone)]
pub struct SyncedMessage {
    /// Message ID.
    pub id: String,
    /// Whether message is unread.
    pub unread: bool,
    /// Message content (text).
    pub content: Option<String>,
    /// Whether message was sent by current user.
    pub outgoing: bool,
    /// Message timestamp.
    pub timestamp: u64,
    /// Sender JID.
    pub sender_jid: String,
    /// Sender push name.
    pub sender_name: Option<String>,
}

/// Delete the `WhatsApp` database files to clear stored credentials.
fn clear_whatsapp_credentials() {
    let db_path = DATA_DIR.join("whatsapp.db");
    let wal_path = format!("{}-wal", db_path.display());
    let shm_path = format!("{}-shm", db_path.display());

    for path in [
        db_path.as_path(),
        Path::new(&wal_path),
        Path::new(&shm_path),
    ] {
        if path.exists()
            && let Err(e) = fs::remove_file(path)
        {
            tracing::warn!("Failed to delete {}: {}", path.display(), e);
        }
    }
}

/// Extract synced messages from a conversation's message list.
/// Shared between `ProcessJoinedGroup` and `ProcessHistorySync`.
fn extract_synced_messages(
    conv: &waproto::whatsapp::Conversation,
    chat_jid: &str,
) -> Vec<SyncedMessage> {
    let mut synced_messages = Vec::new();
    for hist_msg in &conv.messages {
        if let Some(web_msg) = &hist_msg.message
            && let Some(msg) = &web_msg.message
        {
            let msg_id = web_msg.key.id.clone().unwrap_or_default();
            let sender_jid = web_msg
                .key
                .participant
                .clone()
                .unwrap_or_else(|| chat_jid.to_string());
            let outgoing = web_msg.key.from_me.unwrap_or(false);
            let timestamp = web_msg.message_timestamp.unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });

            let content = msg
                .conversation
                .clone()
                .filter(|c| !c.is_empty())
                .or_else(|| {
                    msg.extended_text_message
                        .as_ref()
                        .and_then(|e| e.text.clone().filter(|t| !t.is_empty()))
                });

            synced_messages.push(SyncedMessage {
                id: msg_id,
                unread: false,
                content,
                outgoing,
                timestamp,
                sender_jid,
                sender_name: web_msg.push_name.clone().filter(|n| !n.is_empty()),
            });
        }
    }
    synced_messages
}

#[derive(Debug)]
pub enum ClientCommand {
    /// Start the client connection.
    Start,
    /// Stop the client connection.
    Stop,
    /// Restart the client connection.
    Restart,
    /// Client has been successfully connected and authenticated.
    Connected,
    /// Client has been logged out.
    LoggedOut,
    /// Client has been disconnected.
    Disconnected,

    /// Pair the account.
    Pair {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    /// Client has paired successfully.
    PairSuccess,

    /// Fetch avatar for a JID in background.
    FetchAvatar {
        /// Chat JID.
        jid: String,
    },
    /// Process a `JoinedGroup` event (conversation sync) in background.
    ProcessJoinedGroup {
        /// Lazy conversation to parse.
        lazy_conv: Box<LazyConversation>,
    },
    /// Process a `HistorySync` event in background.
    ProcessHistorySync {
        /// History sync protobuf.
        history_sync: Box<waproto::whatsapp::HistorySync>,
    },
}

impl Client {
    /// Update `WhatsApp` client state.
    fn update_state(&mut self, state: ClientState) {
        self.state = state;
    }
}

#[relm4::component(async, pub)]
impl AsyncComponent for Client {
    type Init = Arc<RuntimeCache>;
    type Input = ClientInput;
    type Output = ClientOutput;
    type CommandOutput = ClientCommand;

    view! {
        // This is a non-visual component, no UI needed
        gtk::Box {
            set_visible: false,
        }
    }

    async fn init(
        init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let os_type = os_info::get().os_type().to_string();

        // Initialize avatar cache.
        let avatar_cache = match AvatarCache::new() {
            Ok(cache) => {
                tracing::info!("Avatar cache initialized");
                Some(cache)
            }
            Err(e) => {
                tracing::error!("Failed to initialize avatar cache: {e}");
                None
            }
        };

        let model = Self {
            state: ClientState::Loading,
            handle: Arc::new(Mutex::new(None)),
            os_type,
            avatar_cache: Arc::new(Mutex::new(avatar_cache)),
            runtime_cache: init,
        };

        let widgets = view_output!();

        // Start the client.
        sender.oneshot_command(async { ClientCommand::Start });

        AsyncComponentParts { model, widgets }
    }

    async fn update(
        &mut self,
        input: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match input {
            ClientInput::Start => {
                sender.oneshot_command(async { ClientCommand::Start });
            }
            ClientInput::Stop => {
                sender.oneshot_command(async { ClientCommand::Stop });
            }
            ClientInput::Restart => {
                sender.oneshot_command(async { ClientCommand::Restart });
            }

            ClientInput::PairWithPhoneNumber { phone_number } => {
                let handle = self.handle.lock().await;
                if let Some(client) = handle.as_ref() {
                    // Sanitize the phone number
                    let phone_number = phone_number
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>();

                    if let Err(e) = client
                        .pair_with_code(PairCodeOptions {
                            platform_id: PlatformId::OtherWebClient,
                            phone_number,
                            platform_display: "Desktop (Linux)".to_string(),
                            show_push_notification: true,
                            ..Default::default()
                        })
                        .await
                    {
                        let _ = sender.output(ClientOutput::Error {
                            message: i18n_f!("Failed to pair with phone number: {0}", e),
                        });
                    }
                }
            }

            ClientInput::MarkRead {
                chat_jid,
                sender_jid,
                message_ids,
            } => {
                if !message_ids.is_empty() {
                    let handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        let Ok(jid) = chat_jid.parse::<Jid>() else {
                            tracing::error!("Failed to parse JID: {chat_jid}");
                            return;
                        };

                        let s_jid = if let Some(sender_jid) = sender_jid {
                            let Ok(jid) = sender_jid.parse::<Jid>() else {
                                tracing::error!("Failed to parse JID: {sender_jid}");
                                return;
                            };

                            Some(jid)
                        } else {
                            None
                        };

                        if let Err(e) = client.mark_as_read(&jid, s_jid.as_ref(), message_ids).await
                        {
                            tracing::error!("Failed to mark messages as read: {e}");
                        }
                    }
                }
            }
            ClientInput::SendMessage { mut message } => {
                let handle = self.handle.lock().await;
                if let Some(client) = handle.as_ref() {
                    let Ok(jid) = message.chat_jid.parse::<Jid>() else {
                        tracing::error!("Failed to parse JID: {}", message.chat_jid);
                        return;
                    };

                    match Box::pin(client.send_message(jid, (*message).clone().into())).await {
                        Ok(msg_id) => {
                            // Update the message server id in-place.
                            message.server_id = msg_id;

                            // Update the message in the database.
                            if let Err(e) = message.save().await {
                                tracing::error!("Failed to update message: {}", e);
                            }

                            let _ = sender.output(ClientOutput::MessageSent {
                                chat_jid: message.chat_jid,
                                msg_id: message.local_id,
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to send message: {e}");

                            let _ = sender.output(ClientOutput::MessageFailed {
                                chat_jid: message.chat_jid,
                                msg_id: message.local_id,
                            });
                        }
                    }
                }
            }
            ClientInput::FetchAvatar { jid } => {
                sender.oneshot_command(async move { ClientCommand::FetchAvatar { jid } });
            }

            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            ClientCommand::Start => {
                if !matches!(
                    self.state,
                    ClientState::Connected | ClientState::Connecting | ClientState::Syncing
                ) {
                    // Initialize SQLite backend.
                    let path = DATA_DIR.join("whatsapp.db").to_string_lossy().into_owned();
                    let backend = match SqliteStore::new(&path).await {
                        Ok(store) => Arc::new(store),
                        Err(e) => {
                            tracing::error!("Failed to initialize SQLite storage: {e}");
                            let _ = sender.output(ClientOutput::Error {
                                message: i18n_f!("Database error: {0}", e),
                            });

                            return;
                        }
                    };
                    tracing::info!("SQLite storage initialized successfully");

                    // Get application version from cargo package.
                    let app_version = (
                        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
                        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
                        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
                    );

                    // Create bot with event handler.
                    let sender_clone = sender.clone();
                    let mut bot = Bot::builder()
                        .with_backend(backend)
                        .with_runtime(TokioRuntime)
                        .with_http_client(UreqHttpClient::new())
                        .with_device_props(
                            Some(self.os_type.clone()),
                            Some(AppVersion {
                                primary: Some(app_version.0),
                                secondary: Some(app_version.1),
                                tertiary: Some(app_version.2),
                                ..Default::default()
                            }),
                            Some(PlatformType::Desktop),
                        )
                        .with_transport_factory(TokioWebSocketTransportFactory::new())
                        .on_event(move |event, _client| {
                            let sender = sender_clone.clone();

                            async move {
                                match event {
                                    Event::Connected(_) => {
                                        sender.oneshot_command(async { ClientCommand::Connected });
                                    }
                                    Event::LoggedOut(_) => {
                                        sender.oneshot_command(async { ClientCommand::LoggedOut });
                                    }
                                    Event::Disconnected(_) => {
                                        sender
                                            .oneshot_command(async { ClientCommand::Disconnected });
                                    }

                                    Event::PairingCode { code, timeout } => {
                                        tracing::info!("Pair code received: {}", code);
                                        sender.oneshot_command(async move {
                                            ClientCommand::Pair {
                                                code: Some(code),
                                                qr_code: None,
                                                timeout,
                                            }
                                        });
                                    }
                                    Event::PairingQrCode { code, timeout } => {
                                        tracing::info!("QR code received");
                                        sender.oneshot_command(async move {
                                            ClientCommand::Pair {
                                                code: None,
                                                qr_code: Some(code),
                                                timeout,
                                            }
                                        });
                                    }
                                    Event::PairSuccess(_) => {
                                        sender
                                            .oneshot_command(async { ClientCommand::PairSuccess });
                                    }

                                    Event::Receipt(receipt) => {
                                        let chat_jid = receipt.source.chat.to_string();
                                        let message_ids = receipt.message_ids;

                                        let _ = sender.output(ClientOutput::ReceiptUpdate {
                                            chat_jid,
                                            message_ids,
                                            receipt_type: receipt.r#type,
                                        });
                                    }
                                    Event::Presence(presence) => {
                                        let jid = presence.from.to_string();
                                        let available = !presence.unavailable;
                                        let last_seen = presence.last_seen;

                                        let _ = sender.output(ClientOutput::PresenceUpdate {
                                            jid,
                                            available,
                                            last_seen,
                                        });
                                    }

                                    Event::Message(message, info) => {
                                        let _ = sender.output(ClientOutput::MessageReceived {
                                            info: Box::new(info),
                                            message,
                                        });
                                    }

                                    Event::JoinedGroup(lazy_conv) => {
                                        // Offload conversation parsing to background task
                                        // to avoid blocking the UI thread
                                        sender.oneshot_command(async move {
                                            ClientCommand::ProcessJoinedGroup {
                                                lazy_conv: Box::new(lazy_conv),
                                            }
                                        });
                                    }

                                    Event::HistorySync(history_sync) => {
                                        sender.oneshot_command(async move {
                                            ClientCommand::ProcessHistorySync {
                                                history_sync: Box::new(history_sync),
                                            }
                                        });
                                    }
                                    Event::OfflineSyncPreview(_) => {
                                        tracing::debug!("Offline sync preview received");
                                    }
                                    Event::OfflineSyncCompleted(_) => {
                                        let _ = sender.output(ClientOutput::OfflineSyncCompleted);
                                    }

                                    Event::DeviceListUpdate(_) => {
                                        // Device list updates - not critical for chat sync
                                        tracing::debug!("Device list update received");
                                    }

                                    Event::PinUpdate(update) => {
                                        let _ = sender.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: update.action.pinned,
                                            muted: None,
                                            archived: None,
                                        });
                                    }
                                    Event::MuteUpdate(update) => {
                                        let _ = sender.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: None,
                                            muted: update.action.muted,
                                            archived: None,
                                        });
                                    }
                                    Event::ArchiveUpdate(update) => {
                                        let _ = sender.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: None,
                                            muted: None,
                                            archived: update.action.archived,
                                        });
                                    }
                                    Event::MarkChatAsReadUpdate(update) => {
                                        // Ignore for now - read state is managed locally.
                                        tracing::debug!(
                                            "Mark chat as read update: {} = {:?}",
                                            update.jid,
                                            update.action
                                        );
                                    }

                                    Event::ContactUpdate(contact_update) => {
                                        let jid = contact_update.jid.to_string();
                                        let name = contact_update.action.full_name.clone();
                                        let phone_number = contact_update.jid.user.clone();
                                        let push_name = contact_update.action.first_name.clone();

                                        let _ = sender.output(ClientOutput::ContactUpdate {
                                            jid,
                                            name,
                                            push_name,
                                            phone_number,
                                        });
                                    }

                                    Event::SelfPushNameUpdated(update) => {
                                        let _ = sender.output(ClientOutput::SelfPushNameUpdated {
                                            push_name: update.new_name,
                                        });
                                    }

                                    e => tracing::warn!("Unhandled event type: {e:#?}"),
                                }
                            }
                        })
                        .build()
                        .await
                        .expect("Failed to build client");

                    // Extract client from bot.
                    let client = bot.client();
                    *self.handle.lock().await = Some(client);

                    self.update_state(ClientState::Connecting);

                    // Start the client.
                    match bot.run().await {
                        Ok(handle) => {
                            // Wait client stop in background.
                            relm4::spawn(async move {
                                let _ = handle.await;
                            });
                        }
                        Err(e) => {
                            tracing::error!("Client failed to start: {e}");

                            let message = i18n_f!("Connection failed: {0}", e);
                            self.update_state(ClientState::Error(message.clone()));
                            let _ = sender.output(ClientOutput::Error { message });
                        }
                    }
                }
            }
            ClientCommand::Stop => {
                {
                    let mut handle = self.handle.lock().await;
                    // TODO: graceful shutdown
                    if let Some(client) = handle.as_ref() {
                        client.disconnect().await;

                        // Clear client reference on disconnect.
                        *handle = None;
                    }
                }

                tracing::info!("Disconnected from WhatsApp");
                self.update_state(ClientState::Disconnected);
                let _ = sender.output(ClientOutput::Disconnected);
            }
            ClientCommand::Restart => {
                // Stop the client.
                {
                    let mut handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        client.disconnect().await;
                        *handle = None;
                    }
                }
                tracing::info!("Disconnected from WhatsApp");

                // Clear credentials for a fresh start.
                clear_whatsapp_credentials();

                // Reset the client state.
                self.update_state(ClientState::Loading);

                // Start the client.
                sender.oneshot_command(async { ClientCommand::Start });
            }
            ClientCommand::Connected => {
                tracing::info!("Connected to WhatsApp!");

                // Get connected user's push name.
                let (jid, push_name) = {
                    let handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        (
                            client.get_lid().await.map(|j| j.to_string()),
                            client.get_push_name().await,
                        )
                    } else {
                        (None, i18n!("You!"))
                    }
                };

                self.update_state(ClientState::Connected);
                let _ = sender.output(ClientOutput::Connected { jid, push_name });
            }
            ClientCommand::LoggedOut => {
                tracing::info!("Logged out from WhatsApp");

                // Disconnect and clear client reference.
                {
                    let mut handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        client.disconnect().await;
                        *handle = None;
                    }
                }

                // Clear stale credentials so the next start begins fresh pairing.
                clear_whatsapp_credentials();

                self.update_state(ClientState::LoggedOut);
                let _ = sender.output(ClientOutput::LoggedOut);
            }
            ClientCommand::Disconnected => {
                tracing::info!("Disconnected from WhatsApp");

                // Don't overwrite active connection states from stale disconnect
                // events (e.g. when a previous connection's Disconnected event
                // arrives after a new connection has started).
                if !matches!(
                    self.state,
                    ClientState::Connected | ClientState::Connecting | ClientState::Syncing
                ) {
                    self.update_state(ClientState::Disconnected);
                    let _ = sender.output(ClientOutput::Disconnected);
                }
            }

            ClientCommand::Pair {
                code,
                qr_code,
                timeout,
            } => {
                let code = code.or(match &self.state {
                    ClientState::Pairing { code, .. } => code.clone(),
                    _ => None,
                });
                let qr_code = qr_code.or(match &self.state {
                    ClientState::Pairing { qr_code, .. } => qr_code.clone(),
                    _ => None,
                });

                self.update_state(ClientState::Pairing {
                    code: code.clone(),
                    qr_code: qr_code.clone(),
                    timeout,
                });
                let _ = sender.output(ClientOutput::PairCode {
                    code,
                    qr_code,
                    timeout,
                });
            }
            ClientCommand::PairSuccess => {
                tracing::info!("Pairing successful, syncing...");

                self.update_state(ClientState::Syncing);
                let _ = sender.output(ClientOutput::PairSuccess);
            }

            ClientCommand::FetchAvatar { jid } => {
                // Spawn avatar fetching as a separate task to avoid blocking command queue.
                let avatar_cache = Arc::clone(&self.avatar_cache);
                let client_handle = Arc::clone(&self.handle);
                let sender_clone = sender.clone();

                relm4::spawn(async move {
                    // Check if already cached (release lock immediately after).
                    let cached_path = {
                        let cache_guard = avatar_cache.lock().await;

                        if let Some(cache) = cache_guard.as_ref() {
                            cache.get_cached_path(&jid)
                        } else {
                            tracing::warn!("Avatar cache not available");
                            return;
                        }
                    };

                    if let Some(path) = cached_path {
                        tracing::debug!("Avatar already cached for {jid}");

                        let _ = sender_clone.output(ClientOutput::AvatarUpdate { jid, path });
                        return;
                    }

                    // Get the client handle (clone Arc to release lock).
                    let client = {
                        let handle = client_handle.lock().await;

                        if let Some(c) = handle.as_ref() {
                            Arc::clone(c)
                        } else {
                            tracing::warn!("Client not available for fetching avatar");
                            return;
                        }
                    };

                    // Parse the JID.
                    let Ok(jid_parsed) = jid.parse::<Jid>() else {
                        tracing::error!("Failed to parse JID for avatar fetch: {jid}");
                        return;
                    };

                    // Fetch the profile picture using the contacts feature.
                    let picture = match client
                        .contacts()
                        .get_profile_picture(&jid_parsed, false)
                        .await
                    {
                        Ok(Some(pic)) => pic,
                        Ok(None) => {
                            tracing::debug!("No profile picture available for {jid}");
                            return;
                        }
                        Err(e) => {
                            tracing::error!("Failed to get profile picture for {jid}: {e}");
                            return;
                        }
                    };

                    tracing::info!("Got profile picture URL for {jid}");

                    // Download the avatar using the client's HTTP client.
                    let request = HttpRequest::get(&picture.url);
                    let response = match client.http_client.execute(request).await {
                        Ok(resp) => resp,
                        Err(e) => {
                            tracing::error!("Failed to download avatar for {jid}: {e}");
                            return;
                        }
                    };

                    if response.status_code < 200 || response.status_code >= 300 {
                        tracing::error!(
                            "Failed to download avatar for {jid}: HTTP {}",
                            response.status_code
                        );
                        return;
                    }

                    // Save to cache (acquire lock only for saving).
                    let path = {
                        let cache_guard = avatar_cache.lock().await;
                        if let Some(cache) = cache_guard.as_ref() {
                            match cache.save_avatar(&jid, &response.body) {
                                Ok(p) => p,
                                Err(e) => {
                                    tracing::error!("Failed to save avatar for {jid}: {e}");
                                    return;
                                }
                            }
                        } else {
                            tracing::warn!("Avatar cache not available for saving");
                            return;
                        }
                    };

                    tracing::info!("Avatar downloaded and cached for {jid}");
                    let _ = sender_clone.output(ClientOutput::AvatarUpdate { jid, path });
                });
            }
            ClientCommand::ProcessJoinedGroup { lazy_conv } => {
                // Offload CPU-intensive protobuf parsing to blocking thread.
                let sender_clone = sender.clone();
                relm4::spawn_blocking(move || {
                    // Parse the lazy conversation (this does protobuf decoding - CPU intensive).
                    // Use get_with_messages() because get() strips messages to save memory.
                    if let Some(conv) = lazy_conv.get_with_messages() {
                        let chat_jid = conv.new_jid.clone().unwrap_or_else(|| conv.id.clone());
                        let is_group = chat_jid.ends_with("@g.us");

                        // Extract participants for groups.
                        let mut participants = Vec::new();
                        if is_group {
                            for p in &conv.participant {
                                participants.push((p.user_jid.clone(), None::<String>));
                            }
                        }

                        // Emit chat synced event.
                        let _ = sender_clone.output(ClientOutput::ChatSynced {
                            jid: chat_jid.clone(),
                            name: conv.name.clone(),
                            pinned: conv.pinned.is_some_and(|p| p > 0),
                            archived: conv.archived.unwrap_or(false),
                            unread_count: conv.unread_count,
                            participants,
                            mute_end_time: conv.mute_end_time,
                            last_message_time: conv.last_msg_timestamp,
                        });

                        // Process messages from the conversation.
                        let synced_messages = extract_synced_messages(&conv, &chat_jid);

                        // Emit messages synced event if we have messages.
                        if !synced_messages.is_empty() {
                            let _ = sender_clone.output(ClientOutput::MessagesSynced {
                                chat_jid,
                                messages: synced_messages,
                            });
                        }
                    }
                });
            }
            ClientCommand::ProcessHistorySync { history_sync } => {
                let sender_clone = sender.clone();
                relm4::spawn_blocking(move || {
                    for conv in &history_sync.conversations {
                        let chat_jid = conv.new_jid.clone().unwrap_or_else(|| conv.id.clone());
                        let is_group = chat_jid.ends_with("@g.us");

                        // Extract participants for groups.
                        let mut participants = Vec::new();
                        if is_group {
                            for p in &conv.participant {
                                participants.push((p.user_jid.clone(), None::<String>));
                            }
                        }

                        let _ = sender_clone.output(ClientOutput::ChatSynced {
                            jid: chat_jid.clone(),
                            name: conv.name.clone(),
                            pinned: conv.pinned.is_some_and(|p| p > 0),
                            archived: conv.archived.unwrap_or(false),
                            unread_count: conv.unread_count,
                            participants,
                            mute_end_time: conv.mute_end_time,
                            last_message_time: conv.last_msg_timestamp,
                        });

                        let synced_messages = extract_synced_messages(conv, &chat_jid);

                        if !synced_messages.is_empty() {
                            let _ = sender_clone.output(ClientOutput::MessagesSynced {
                                chat_jid,
                                messages: synced_messages,
                            });
                        }
                    }

                    let _ = sender_clone.output(ClientOutput::HistorySyncCompleted);
                });
            }
        }
    }
}
