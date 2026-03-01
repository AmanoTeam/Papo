use std::{sync::Arc, time::Duration};

use adw::prelude::*;
use chrono::{DateTime, Utc};
use relm4::prelude::*;
use tokio::sync::Mutex;
use wacore::{
    net::HttpRequest,
    pair_code::{PairCodeOptions, PlatformId},
    types::{events::Event, message::MessageInfo},
};
use waproto::whatsapp::{
    Message,
    device_props::{AppVersion, PlatformType},
};
use whatsapp_rust::{Jid, bot::Bot, store::SqliteStore};
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;

use crate::{
    DATA_DIR, i18n,
    session::{AvatarCache, RuntimeCache},
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
    avatar_cache: Arc<tokio::sync::Mutex<Option<AvatarCache>>>,
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
    /// Send a text message.
    SendMessage {
        /// Target JID (e.g., "1234567890@s.whatsapp.net").
        jid: String,
        /// The content of the message.
        text: String,
    },
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

    /// Read receipts updated.
    ReadReceipts {
        chat_jid: String,
        message_ids: Vec<String>,
    },
    /// User presence updated.
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<DateTime<Utc>>,
    },

    /// Message was sent successfully.
    MessageSent { id: String },
    /// Message failed to send.
    MessageFailed { id: String, error: String },
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
        /// Whether chat is archived.
        archived: bool,
        /// Whether chat is pinned.
        pinned: bool,
        /// Mute end time (if muted).
        mute_end_time: Option<u64>,
        /// Last message timestamp.
        last_message_time: Option<u64>,
        /// Unread message count.
        unread_count: Option<u32>,
        /// Group participants (for groups).
        participants: Vec<(String, Option<String>)>,
    },
    /// Messages synced from history for a chat.
    MessagesSynced {
        /// Chat JID.
        chat_jid: String,
        /// Synced messages.
        messages: Vec<SyncedMessage>,
    },

    /// Contact updated (from sync or individual update).
    ContactUpdated {
        /// Contact JID.
        jid: String,
        /// Full name from address book.
        name: Option<String>,
        /// Phone number (from JID user part).
        phone_number: String,
        /// Push name (first name).
        push_name: Option<String>,
    },

    /// Avatar updated for a chat.
    AvatarUpdated {
        /// Chat JID.
        jid: String,
        /// Path to the cached avatar image.
        path: String,
    },

    /// Error occurred.
    Error { message: String },
}

/// A message synced from history.
#[derive(Debug, Clone)]
pub struct SyncedMessage {
    /// Message ID.
    pub id: String,
    /// Sender JID.
    pub sender_jid: String,
    /// Sender push name.
    pub sender_name: Option<String>,
    /// Message content (text).
    pub content: Option<String>,
    /// Whether message was sent by current user.
    pub outgoing: bool,
    /// Message timestamp.
    pub timestamp: u64,
    /// Whether message is unread.
    pub unread: bool,
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

    /// Process a `JoinedGroup` event (conversation sync) in background.
    ProcessJoinedGroup {
        /// Lazy conversation to parse.
        lazy_conv: wacore::types::events::LazyConversation,
    },
    /// Fetch avatar for a JID in background.
    FetchAvatar {
        /// Chat JID.
        jid: String,
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

        // Initialize avatar cache
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
            runtime_cache: init,
            avatar_cache: Arc::new(tokio::sync::Mutex::new(avatar_cache)),
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
                            custom_code: None,
                            platform_id: PlatformId::OtherWebClient,
                            phone_number,
                            show_push_notification: true,
                            ..Default::default()
                        })
                        .await
                    {
                        let _ = sender.output(ClientOutput::Error {
                            message: format!("Failed to pair with phone number: {e}"),
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

            ClientInput::FetchAvatar { jid } => {
                sender.oneshot_command(async move { ClientCommand::FetchAvatar { jid } });
            }

            // TODO: Implement these call and typing features
            ClientInput::StartCall {
                jid: _,
                is_video: _,
            } => {
                tracing::warn!("StartCall not yet implemented");
            }
            ClientInput::AcceptCall { call_id: _ } => {
                tracing::warn!("AcceptCall not yet implemented");
            }
            ClientInput::DeclineCall { call_id: _ } => {
                tracing::warn!("DeclineCall not yet implemented");
            }
            ClientInput::SendTyping { jid: _ } => {
                tracing::warn!("SendTyping not yet implemented");
            }
            ClientInput::StopTyping { jid: _ } => {
                tracing::warn!("StopTyping not yet implemented");
            }
            ClientInput::SendMessage { jid: _, text: _ } => {
                tracing::warn!("SendMessage not yet implemented");
            }
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
                                message: format!("Database error: {e}"),
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

                                        let _ = sender.output(ClientOutput::ReadReceipts {
                                            chat_jid,
                                            message_ids,
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
                                            ClientCommand::ProcessJoinedGroup { lazy_conv }
                                        });
                                    }

                                    Event::HistorySync(_)
                                    | Event::OfflineSyncPreview(_)
                                    | Event::OfflineSyncCompleted(_) => {
                                        // History sync events - already handled via JoinedGroup
                                        tracing::debug!("History sync event received");
                                    }

                                    Event::DeviceListUpdate(_) => {
                                        // Device list updates - not critical for chat sync
                                        tracing::debug!("Device list update received");
                                    }

                                    Event::ContactUpdate(contact_update) => {
                                        let jid = contact_update.jid.to_string();
                                        let name = contact_update.action.full_name.clone();
                                        let phone_number = contact_update.jid.user.clone();
                                        let push_name = contact_update.action.first_name.clone();

                                        let _ = sender.output(ClientOutput::ContactUpdated {
                                            jid,
                                            name,
                                            phone_number,
                                            push_name,
                                        });
                                    }

                                    e => tracing::warn!("Unhandled event type: {e:#?}"),
                                }
                            }
                        })
                        .build()
                        .await
                        .expect("Failed to build bot");

                    // Extract client from bot.
                    let client = bot.client();
                    *self.handle.lock().await = Some(client);

                    self.update_state(ClientState::Connecting);

                    // Runs the bot.
                    if let Err(e) = bot.run().await {
                        tracing::error!("Bot failed to start: {e}");

                        let message = format!("Connection failed: {e}");
                        self.update_state(ClientState::Error(message.clone()));
                        let _ = sender.output(ClientOutput::Error { message });
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
                sender.oneshot_command(async { ClientCommand::Stop });

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
                            client.get_pn().await.map(|j| j.to_string()),
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

                self.update_state(ClientState::LoggedOut);
                let _ = sender.output(ClientOutput::LoggedOut);
            }
            ClientCommand::Disconnected => {
                tracing::info!("Disconnected from WhatsApp");

                self.update_state(ClientState::Disconnected);
                let _ = sender.output(ClientOutput::Disconnected);
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

            ClientCommand::ProcessJoinedGroup { lazy_conv } => {
                // Offload CPU-intensive protobuf parsing to blocking thread
                let sender_clone = sender.clone();
                relm4::spawn_blocking(move || {
                    // Parse the lazy conversation (this does protobuf decoding - CPU intensive)
                    if let Some(conv) = lazy_conv.get() {
                        let chat_jid = conv.new_jid.clone().unwrap_or_else(|| conv.id.clone());
                        let is_group = chat_jid.ends_with("@g.us");

                        // Extract participants for groups
                        let mut participants = Vec::new();
                        if is_group {
                            for p in &conv.participant {
                                participants.push((p.user_jid.clone(), None::<String>));
                            }
                        }

                        // Emit chat synced event
                        let _ = sender_clone.output(ClientOutput::ChatSynced {
                            jid: chat_jid.clone(),
                            name: conv.name.clone(),
                            archived: conv.archived.unwrap_or(false),
                            pinned: conv.pinned.is_some_and(|p| p > 0),
                            mute_end_time: conv.mute_end_time,
                            last_message_time: conv.last_msg_timestamp,
                            unread_count: conv.unread_count,
                            participants,
                        });

                        // Process messages from the conversation
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
                                    .unwrap_or_else(|| chat_jid.clone());
                                let outgoing = web_msg.key.from_me.unwrap_or(false);
                                let timestamp = web_msg.message_timestamp.unwrap_or_else(|| {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs()
                                });

                                // Extract message content
                                let content = msg.conversation.clone().filter(|c| !c.is_empty());

                                synced_messages.push(SyncedMessage {
                                    id: msg_id,
                                    sender_jid,
                                    sender_name: web_msg
                                        .push_name
                                        .clone()
                                        .filter(|n| !n.is_empty()),
                                    content,
                                    outgoing,
                                    timestamp,
                                    unread: false,
                                });
                            }
                        }

                        // Emit messages synced event if we have messages
                        if !synced_messages.is_empty() {
                            let _ = sender_clone.output(ClientOutput::MessagesSynced {
                                chat_jid,
                                messages: synced_messages,
                            });
                        }
                    }
                });
            }
            ClientCommand::FetchAvatar { jid } => {
                // Spawn avatar fetching as a separate task to avoid blocking command queue
                let avatar_cache = Arc::clone(&self.avatar_cache);
                let client_handle = Arc::clone(&self.handle);
                let sender_clone = sender.clone();

                relm4::spawn(async move {
                    // Check if already cached (release lock immediately after)
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
                        let _ = sender_clone.output(ClientOutput::AvatarUpdated { jid, path });
                        return;
                    }

                    // Get the client handle (clone Arc to release lock)
                    let client = {
                        let handle = client_handle.lock().await;
                        if let Some(c) = handle.as_ref() {
                            Arc::clone(c)
                        } else {
                            tracing::warn!("Client not available for fetching avatar");
                            return;
                        }
                    };

                    // Parse the JID
                    let Ok(jid_parsed) = jid.parse::<Jid>() else {
                        tracing::error!("Failed to parse JID for avatar fetch: {jid}");
                        return;
                    };

                    // Fetch the profile picture using the contacts feature
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

                    // Download the avatar using the client's HTTP client
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

                    // Save to cache (acquire lock only for saving)
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
                    let _ = sender_clone.output(ClientOutput::AvatarUpdated { jid, path });
                });
            }
        }
    }
}
