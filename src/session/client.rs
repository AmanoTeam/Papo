use std::{collections::HashSet, sync::Arc, time::Duration};

use adw::prelude::*;
use chrono::{DateTime, Utc};
use relm4::prelude::*;
use tokio::sync::Mutex;
use uuid::Uuid;
use wacore::{store::traits::DeviceStore, types::presence::ReceiptType};
use waepic::{
    ClientConfiguration, Jid, LoginStatus, PairEvent, SqliteSession, Update,
    update::SyncedConversation, wacore,
};

use crate::{DATA_DIR, i18n, i18n_f, session::AvatarCache, state::ChatMessage};

/// Shared client handle for accessing the `WhatsApp` client.
pub type ClientHandle = Arc<Mutex<Option<waepic::Client>>>;

/// `WhatsApp` client wrapper that manages the connection and provides
/// a clean interface for UI operations.
#[derive(Clone)]
pub struct Client {
    /// Client connection state.
    pub state: ClientState,
    /// Shared client reference.
    handle: ClientHandle,

    /// Avatar cache for downloading and storing profile pictures.
    avatar_cache: Arc<Mutex<Option<AvatarCache>>>,
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

    /// Pair scanning a QR code.
    PairWithQrCode,
    /// Pair with a phone number.
    PairWithPhoneNumber { phone_number: String },

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
    MessageReceived { message: Box<waepic::Message> },

    /// Chat synced from history.
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
}

impl Client {
    /// Update `WhatsApp` client state.
    fn update_state(&mut self, state: ClientState) {
        self.state = state;
    }
}

/// Convert a waepic `SyncedConversation` into `SyncedMessage` items.
fn convert_synced_conversation(conv: &SyncedConversation) -> (ClientOutput, Vec<SyncedMessage>) {
    let jid = conv.jid.to_string();
    let is_group = jid.ends_with("@g.us");

    // Extract participants for groups.
    let participants = if is_group {
        conv.messages
            .iter()
            .filter(|m| !m.outgoing())
            .map(|m| (m.sender().to_string(), None::<String>))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };

    let last_message_time = conv.messages.last().map(waepic::Message::date);

    let chat_synced = ClientOutput::ChatSynced {
        jid,
        name: conv.name.clone(),
        pinned: conv.pinned,
        archived: conv.archived,
        unread_count: Some(conv.unread_count),
        participants,
        mute_end_time: None,
        last_message_time,
    };

    let messages = conv
        .messages
        .iter()
        .map(|m| SyncedMessage {
            id: m.id().to_string(),
            unread: false,
            content: m.text().map(ToString::to_string),
            outgoing: m.outgoing(),
            timestamp: m.date(),
            sender_jid: m.sender().to_string(),
            sender_name: None,
        })
        .collect::<Vec<SyncedMessage>>();

    (chat_synced, messages)
}

#[relm4::component(async, pub)]
impl AsyncComponent for Client {
    type Init = ();
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
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
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
            avatar_cache: Arc::new(Mutex::new(avatar_cache)),
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

            ClientInput::PairWithQrCode => {
                let stream = {
                    let handle = self.handle.lock().await;
                    match handle.as_ref() {
                        Some(client) => client.request_pairing().await,
                        None => return,
                    }
                };

                match stream {
                    Ok(mut stream) => {
                        let sender = sender.clone();
                        relm4::spawn(async move {
                            while let Some(event) = stream.recv().await {
                                match event {
                                    PairEvent::QrCode { code, timeout } => {
                                        sender.oneshot_command(async move {
                                            ClientCommand::Pair {
                                                code: None,
                                                qr_code: Some(code),
                                                timeout: Duration::from_secs(timeout),
                                            }
                                        });
                                    }
                                    PairEvent::Success => {}
                                    PairEvent::Error(e) => {
                                        tracing::warn!("QR pairing stream error: {e}");
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        let _ = sender.output(ClientOutput::Error {
                            message: i18n_f!("Failed to pair with qr code: {0}", e),
                        });
                    }
                }
            }
            ClientInput::PairWithPhoneNumber { phone_number } => {
                let handle = self.handle.lock().await;
                if let Some(client) = handle.as_ref() {
                    // Sanitize the phone number
                    let phone_number = phone_number
                        .chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>();

                    match client.request_pair_code(&phone_number).await {
                        Ok(code) => {
                            sender.oneshot_command(async move {
                                ClientCommand::Pair {
                                    code: Some(code),
                                    qr_code: None,
                                    timeout: Duration::from_secs(180),
                                }
                            });
                        }
                        Err(e) => {
                            let _ = sender.output(ClientOutput::Error {
                                message: i18n_f!("Failed to pair with phone number: {0}", e),
                            });
                        }
                    }
                }
            }

            ClientInput::MarkRead {
                chat_jid,
                sender_jid: _,
                message_ids,
            } => {
                if !message_ids.is_empty() {
                    let handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        let Ok(jid) = chat_jid.parse::<Jid>() else {
                            tracing::error!("Failed to parse JID: {chat_jid}");
                            return;
                        };

                        let refs = message_ids
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<&str>>();
                        let chat = client.chat(jid);
                        if let Err(e) = client.mark_as_read(chat, &refs).await {
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

                    let chat = client.chat(jid);
                    match Box::pin(client.send_message(chat, message.content.as_str())).await {
                        Ok(msg) => {
                            // Update the message server id in-place.
                            message.server_id = msg.id().to_string();

                            // Update the message in the database.
                            if let Err(e) = message.save().await {
                                tracing::error!("Failed to update message: {e}");
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
                    let session = match SqliteSession::new(DATA_DIR.join("session.db")) {
                        Ok(session) => Arc::new(session),
                        Err(e) => {
                            tracing::error!("Failed to initialize session storage: {e}");
                            let message = i18n_f!("Failed to initialize session storage: {0}", e);
                            self.update_state(ClientState::Error(message.clone()));
                            let _ = sender.output(ClientOutput::Error { message });
                            return;
                        }
                    };
                    if let Err(e) = DeviceStore::create(&*session).await {
                        tracing::error!("Failed to create session: {e}");
                        return;
                    }

                    let (client, runner) = waepic::Client::connect(
                        session,
                        ClientConfiguration {
                            auto_history_sync: true,
                            ..Default::default()
                        },
                    );

                    if let Err(e) = client.load_or_create_device().await {
                        tracing::error!("Failed to load device: {e}");
                        let message = i18n_f!("Failed to initialize device: {0}", e);
                        self.update_state(ClientState::Error(message.clone()));
                        let _ = sender.output(ClientOutput::Error { message });
                        return;
                    }

                    let sender_runner = sender.clone();
                    relm4::spawn(async move {
                        if let Err(e) = runner.run().await {
                            tracing::error!("Connection runner failed: {e}");
                            sender_runner.oneshot_command(async { ClientCommand::LoggedOut });
                        }
                    });

                    let (mut stream, future) = match client.stream_updates() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to get update stream: {e}");
                            let message = i18n_f!("Failed to start update stream: {0}", e);
                            self.update_state(ClientState::Error(message.clone()));
                            let _ = sender.output(ClientOutput::Error { message });
                            return;
                        }
                    };

                    *self.handle.lock().await = Some(client);
                    self.update_state(ClientState::Connecting);
                    relm4::spawn(future);

                    let sender_stream = sender.clone();
                    relm4::spawn(async move {
                        while let Some(update) = stream.next().await {
                            match update {
                                Update::Connected => {
                                    sender_stream
                                        .oneshot_command(async { ClientCommand::Connected });
                                }
                                Update::LoggedOut => {
                                    sender_stream
                                        .oneshot_command(async { ClientCommand::LoggedOut });
                                }
                                Update::Disconnected => {
                                    sender_stream
                                        .oneshot_command(async { ClientCommand::Disconnected });
                                }

                                Update::PairingCode { code, timeout } => {
                                    sender_stream.oneshot_command(async move {
                                        ClientCommand::Pair {
                                            code: Some(code),
                                            qr_code: None,
                                            timeout: Duration::from_secs(timeout),
                                        }
                                    });
                                }
                                Update::PairingQrCode { code, timeout } => {
                                    sender_stream.oneshot_command(async move {
                                        ClientCommand::Pair {
                                            code: None,
                                            qr_code: Some(code),
                                            timeout: Duration::from_secs(timeout),
                                        }
                                    });
                                }
                                Update::PairSuccess => {
                                    sender_stream
                                        .oneshot_command(async { ClientCommand::PairSuccess });
                                }

                                Update::Receipt(receipt) => {
                                    let _ = sender_stream.output(ClientOutput::ReceiptUpdate {
                                        chat_jid: receipt.chat.to_string(),
                                        message_ids: receipt.message_ids,
                                        receipt_type: receipt.receipt_type,
                                    });
                                }
                                Update::Presence(presence) => {
                                    let last_seen = presence.last_seen.and_then(|ts| {
                                        DateTime::from_timestamp(ts.cast_signed(), 0)
                                    });
                                    let _ = sender_stream.output(ClientOutput::PresenceUpdate {
                                        jid: presence.chat.to_string(),
                                        available: presence.available,
                                        last_seen,
                                    });
                                }

                                Update::NewMessage(message) => {
                                    let _ = sender_stream.output(ClientOutput::MessageReceived {
                                        message: Box::new(message),
                                    });
                                }

                                Update::HistorySync(chunk) => {
                                    for conv in &chunk.conversations {
                                        let (chat_synced, messages) =
                                            convert_synced_conversation(conv);
                                        let _ = sender_stream.output(chat_synced);
                                        if !messages.is_empty() {
                                            let _ = sender_stream.output(
                                                ClientOutput::MessagesSynced {
                                                    chat_jid: conv.jid.to_string(),
                                                    messages,
                                                },
                                            );
                                        }
                                    }
                                }
                                Update::HistorySyncCompleted => {
                                    let _ =
                                        sender_stream.output(ClientOutput::HistorySyncCompleted);
                                }
                                Update::OfflineSyncPreview { .. } => {
                                    tracing::debug!("Offline sync preview received");
                                }
                                Update::OfflineSyncCompleted { .. } => {
                                    let _ =
                                        sender_stream.output(ClientOutput::OfflineSyncCompleted);
                                }

                                Update::DeviceListUpdate(_) => {
                                    tracing::debug!("Device list update received");
                                }

                                Update::PinUpdate(update) => {
                                    let _ =
                                        sender_stream.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: Some(update.on),
                                            muted: None,
                                            archived: None,
                                        });
                                }
                                Update::MuteUpdate(update) => {
                                    let _ =
                                        sender_stream.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: None,
                                            muted: Some(update.on),
                                            archived: None,
                                        });
                                }
                                Update::ArchiveUpdate(update) => {
                                    let _ =
                                        sender_stream.output(ClientOutput::ChatPropertyUpdate {
                                            jid: update.jid.to_string(),
                                            pinned: None,
                                            muted: None,
                                            archived: Some(update.on),
                                        });
                                }
                                Update::MarkChatAsReadUpdate(update) => {
                                    tracing::debug!(
                                        "Mark chat as read update: {} = {:?}",
                                        update.jid,
                                        update.on
                                    );
                                }

                                Update::ContactUpdate(contact) => {
                                    let jid_str = contact.jid.to_string();
                                    let _ = sender_stream.output(ClientOutput::ContactUpdate {
                                        phone_number: jid_str.clone(),
                                        jid: jid_str,
                                        name: contact.name,
                                        push_name: contact.push_name,
                                    });
                                }

                                Update::SelfPushNameUpdated(update) => {
                                    let _ =
                                        sender_stream.output(ClientOutput::SelfPushNameUpdated {
                                            push_name: update.new_name,
                                        });
                                }

                                Update::StreamReplaced => {
                                    tracing::info!("Stream replaced, waiting for reconnection...");
                                }
                                Update::ConnectFailure(failure) => {
                                    tracing::error!("Connection failure: {failure:?}");
                                    let _ = sender_stream.output(ClientOutput::Error {
                                        message: i18n_f!("Connection failed: {0}", failure.message),
                                    });
                                }
                                Update::TemporaryBan(ban) => {
                                    tracing::error!("Temporary ban: code={}", ban.code);
                                    let _ = sender_stream.output(ClientOutput::Error {
                                        message: i18n!("Your account has been temporarily banned."),
                                    });
                                }
                                Update::ClientOutdated => {
                                    let _ = sender_stream.output(ClientOutput::Error {
                                        message: i18n!("Your client is outdated. Please update."),
                                    });
                                }
                                Update::StreamError(error) => {
                                    tracing::error!("Stream error: {}", error.code);
                                    let _ = sender_stream.output(ClientOutput::Error {
                                        message: i18n_f!("Stream error: {0}", error.code),
                                    });
                                }

                                e => tracing::warn!("Unhandled update: {e:#?}"),
                            }
                        }
                    });
                }
            }
            ClientCommand::Stop => {
                {
                    let mut handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        if let Err(e) = client.disconnect().await {
                            tracing::error!("Failed to disconnect: {e}");
                        }

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
                        if let Err(e) = client.disconnect().await {
                            tracing::error!("Failed to disconnect: {e}");
                        }
                        *handle = None;
                    }
                }
                tracing::info!("Disconnected from WhatsApp");

                // Reset the client state.
                self.update_state(ClientState::Loading);

                // Start the client.
                sender.oneshot_command(async { ClientCommand::Start });
            }
            ClientCommand::Connected => {
                tracing::info!("Connected to WhatsApp!");
                self.update_state(ClientState::Connected);

                let handle = self.handle.lock().await;
                if let Some(client) = handle.as_ref() {
                    match client.check_login_status().await {
                        Ok(LoginStatus::Authorized { jid, push_name }) => {
                            let _ = sender.output(ClientOutput::Connected {
                                jid: Some(jid.to_string()),
                                push_name,
                            });
                        }
                        Ok(LoginStatus::NotAuthorized) => {
                            sender.input(ClientInput::PairWithQrCode);
                        }
                        Err(e) => {
                            tracing::error!("Failed to check login status: {e}");
                        }
                    }
                }
            }
            ClientCommand::LoggedOut => {
                tracing::info!("Logged out from WhatsApp");

                // Disconnect and clear client reference.
                {
                    let mut handle = self.handle.lock().await;
                    if let Some(client) = handle.as_ref() {
                        if let Err(e) = client.disconnect().await {
                            tracing::error!("Failed to disconnect: {e}");
                        }
                        *handle = None;
                    }
                }

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

                sender.oneshot_command(async { ClientCommand::Connected });

                self.update_state(ClientState::Syncing);
                let _ = sender.output(ClientOutput::PairSuccess);
            }

            ClientCommand::FetchAvatar { jid } => {
                // Avatar fetching is not yet supported with waepic (no contacts/http_client API).
                tracing::debug!("Avatar fetch requested for {jid} (not yet supported)");
            }
        }
    }
}
