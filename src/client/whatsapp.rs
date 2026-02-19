use std::{sync::Arc, time::Duration};

use adw::prelude::*;
use relm4::prelude::*;
use tokio::sync::Mutex;
use wacore::{
    pair_code::{PairCodeOptions, PlatformId},
    types::{events::Event, message::MessageInfo},
};
use waproto::whatsapp::device_props::{AppVersion, PlatformType};
use whatsapp_rust::{bot::Bot, store::SqliteStore};
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;

use crate::config::DATABASE_PATH;

/// Shared client handle for accessing the WhatsApp client.
pub type ClientHandle = Arc<Mutex<Option<Arc<whatsapp_rust::Client>>>>;

/// WhatsApp client wrapper that manages the connection and provides
/// a clean interface for UI operations.
#[derive(Clone)]
pub struct Client {
    /// Client connection state.
    pub state: ClientState,
    /// Shared client reference.
    handle: ClientHandle,
    /// System OS type.
    os_type: String,
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
        message_ids: Vec<String>,
    },
    /// Send a text message.
    SendMessage {
        /// Target JID (e.g., "1234567890@s.whatsapp.net").
        jid: String,
        /// The content of the message.
        text: String,
    },
}

#[derive(Debug)]
pub enum ClientOutput {
    /// Client is loading.
    Loading,
    /// Client has been successfully connected and authenticated.
    Connected,
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

    /// Message was sent successfully.
    MessageSent { id: String },
    /// Message failed to send.
    MessageFailed { id: String, error: String },
    /// New message received.
    MessageReceived { message: Box<Message> },

    /// Error occurred.
    Error { message: String },
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
}

/// Wrapper for message data.
#[derive(Debug, Clone)]
pub struct Message {
    pub info: MessageInfo,
    pub content: Arc<waproto::whatsapp::Message>,
}

impl Client {
    /// Updates WhatsApp client state.
    fn update_state(&mut self, state: ClientState) {
        self.state = state;
    }
}

#[relm4::component(async, pub)]
impl AsyncComponent for Client {
    type Init = ();
    type Input = ClientInput;
    type Output = ClientOutput;
    type CommandOutput = ClientCommand;

    view! {
        gtk::Box {
            set_visible: false,
        }
    }

    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let os_type = os_info::get().os_type().to_string();

        let model = Self {
            state: ClientState::Loading,
            handle: Arc::new(Mutex::new(None)),
            os_type,
        };

        let widgets = view_output!();

        // Start the client.
        relm4::spawn_local(async move {
            sender.oneshot_command(async { ClientCommand::Start });
        });

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
                if let Some(client) = handle.clone() {
                    // Sanitize the phone number
                    let phone_number = phone_number
                        .chars()
                        .filter(|c| c.is_ascii_digit())
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
                            message: format!("Failed to pair with phone number: {}", e),
                        });
                    }
                }
            }

            _ => {}
        }
    }

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
                    let backend = match SqliteStore::new(DATABASE_PATH).await {
                        Ok(store) => Arc::new(store),
                        Err(e) => {
                            tracing::error!("Failed to initialize SQLite storage: {}", e);
                            let _ = sender.output(ClientOutput::Error {
                                message: format!("Database error: {}", e),
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

                                    e => tracing::warn!("Unhandled event type: {:?}", e),
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
                        tracing::error!("Bot failed to start: {}", e);

                        let message = format!("Connection failed: {}", e);
                        self.update_state(ClientState::Error(message.clone()));
                        let _ = sender.output(ClientOutput::Error { message });
                    }
                }
            }
            ClientCommand::Stop => {
                {
                    let mut handle = self.handle.lock().await;

                    // TODO: graceful shutdown
                    if let Some(client) = handle.clone() {
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

                self.update_state(ClientState::Connected);
                let _ = sender.output(ClientOutput::Connected);
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
        }
    }
}
