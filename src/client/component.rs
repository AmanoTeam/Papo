//! WhatsApp client component for Relm4
//!
//! This component wraps the whatsapp-rust client and provides a Relm4-native interface
//! for connection management, messaging, and event handling.

use std::path::PathBuf;
use std::sync::Arc;

use relm4::prelude::*;
use tokio::sync::{Mutex, mpsc};
use whatsapp_rust::bot::{Bot, ControlFlow};
use whatsapp_rust::calls::CallOptions;
use whatsapp_rust::client::Client;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust::types::message::Message;
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;

/// Shared client handle for accessing the WhatsApp client from UI
pub type ClientHandle = Arc<Mutex<Option<Arc<Client>>>>;

/// Events emitted by the WhatsApp client to the UI
#[derive(Debug, Clone)]
pub enum WhatsAppClientEvent {
    /// Client is connecting
    Connecting,
    /// QR code received for pairing
    QrCode { url: String, code: String },
    /// 8-character pairing code received (for phone number linking)
    PairCode { code: String },
    /// Client successfully connected and authenticated
    Connected {
        phone_number: String,
        pushname: String,
    },
    /// Client disconnected
    Disconnected,
    /// New message received
    MessageReceived { message: Message },
    /// Message was sent successfully
    MessageSent { id: String },
    /// Message failed to send
    MessageFailed { id: String, error: String },
    /// Read receipts updated
    ReadReceipts { chat_jid: String, message_ids: Vec<String> },
    /// Incoming call offer
    CallOffer {
        call_id: String,
        from_jid: String,
        is_video: bool,
    },
    /// Call ended
    CallEnded { call_id: String },
    /// Error occurred
    Error { message: String },
}

/// Internal events from background tasks
#[derive(Debug)]
pub enum InternalEvent {
    /// Client event from the bot
    ClientEvent(whatsapp_rust::types::events::Event),
    /// Connection task completed
    ConnectionResult(Result<(), String>),
    /// Message send result
    SendResult(Result<String, String>),
}

/// Input messages to control the client
#[derive(Debug)]
pub enum WhatsAppClientInput {
    /// Start the client connection
    Start {
        /// Path to store SQLite database
        store_path: PathBuf,
    },
    /// Stop the client connection
    Stop,
    /// Send a text message
    SendMessage {
        /// Target JID (e.g., "1234567890@s.whatsapp.net")
        jid: String,
        /// Message text
        text: String,
    },
    /// Mark messages as read
    MarkRead {
        chat_jid: String,
        message_ids: Vec<String>,
    },
    /// Send typing indicator
    SendTyping { jid: String },
    /// Stop typing indicator
    StopTyping { jid: String },
    /// Accept incoming call
    AcceptCall { call_id: String },
    /// Decline incoming call
    DeclineCall { call_id: String },
    /// Start a new call
    StartCall {
        jid: String,
        is_video: bool,
    },
}

/// WhatsApp client component that manages the connection lifecycle
pub struct WhatsAppClientComponent {
    /// Shared client reference
    client_handle: ClientHandle,
    /// Client connection state
    state: ClientState,
    /// Event sender for forwarding events
    event_sender: Option<mpsc::UnboundedSender<WhatsAppClientEvent>>,
}

/// Current state of the client connection
#[derive(Debug, Clone, PartialEq)]
pub enum ClientState {
    /// Not connected
    Disconnected,
    /// Connecting in progress
    Connecting,
    /// Connected and authenticated
    Connected,
    /// Error state
    Error(String),
}

impl Default for ClientState {
    fn default() -> Self {
        ClientState::Disconnected
    }
}

impl WhatsAppClientComponent {
    /// Create a new client component
    pub fn new() -> Self {
        Self {
            client_handle: Arc::new(Mutex::new(None)),
            state: ClientState::Disconnected,
            event_sender: None,
        }
    }

    /// Get the client handle
    pub fn handle(&self) -> ClientHandle {
        self.client_handle.clone()
    }

    /// Get current connection state
    pub fn state(&self) -> &ClientState {
        &self.state
    }

    /// Run the client connection in background
    async fn run_client(
        store_path: PathBuf,
        event_tx: mpsc::UnboundedSender<WhatsAppClientEvent>,
        client_handle: ClientHandle,
    ) -> Result<(), String> {
        // Initialize store
        let store = SqliteStore::new(store_path)
            .await
            .map_err(|e| format!("Failed to initialize store: {}", e))?;

        // Create bot with event handler
        let event_tx_clone = event_tx.clone();
        let bot = Bot::builder()
            .store(store)
            .transport_factory(TokioWebSocketTransportFactory::new())
            .http_client(UreqHttpClient::new())
            .on_event(move |event| {
                let _ = event_tx_clone.send(WhatsAppClientEvent::from(event.clone()));
                ControlFlow::Continue
            })
            .build()
            .await
            .map_err(|e| format!("Failed to build bot: {}", e))?;

        // Store client reference
        let client = bot.client();
        *client_handle.lock().await = Some(client.clone());

        // Notify connection started
        let _ = event_tx.send(WhatsAppClientEvent::Connecting);

        // Run the bot (this blocks until disconnect)
        bot.run()
            .await
            .map_err(|e| format!("Bot error: {}", e))?;

        // Clear client reference on disconnect
        *client_handle.lock().await = None;
        let _ = event_tx.send(WhatsAppClientEvent::Disconnected);

        Ok(())
    }

    /// Send a message using the client
    async fn send_message(
        client_handle: ClientHandle,
        jid: String,
        text: String,
    ) -> Result<String, String> {
        let client = client_handle
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("Client not connected")?;

        // Parse JID
        let jid: wacore_binary::jid::Jid = jid
            .parse()
            .map_err(|e| format!("Invalid JID: {:?}", e))?;

        // Build text message protobuf
        let message = waproto::whatsapp::Message {
            conversation: text,
            ..Default::default()
        };

        // Send message
        let message_id = client
            .send_message(jid, message)
            .await
            .map_err(|e| format!("Failed to send message: {}", e))?;

        Ok(message_id)
    }

    /// Send typing indicator
    async fn send_typing(
        client_handle: ClientHandle,
        jid: String,
        active: bool,
    ) -> Result<(), String> {
        let client = client_handle
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("Client not connected")?;

        let jid: wacore_binary::jid::Jid = jid
            .parse()
            .map_err(|e| format!("Invalid JID: {:?}", e))?;

        if active {
            client
                .chatstate()
                .send_composing(&jid)
                .await
                .map_err(|e| format!("Failed to send typing: {}", e))?;
        } else {
            client
                .chatstate()
                .send_paused(&jid)
                .await
                .map_err(|e| format!("Failed to send paused: {}", e))?;
        }

        Ok(())
    }

    /// Mark messages as read
    async fn mark_read(
        client_handle: ClientHandle,
        _chat_jid: String,
        _message_ids: Vec<String>,
    ) -> Result<(), String> {
        let _client = client_handle
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("Client not connected")?;

        // Note: Read receipts implementation depends on whatsapp-rust API availability
        // This is a placeholder for future implementation
        Ok(())
    }
}

impl Default for WhatsAppClientComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for WhatsAppClientComponent {
    type Input = WhatsAppClientInput;
    type Output = WhatsAppClientEvent;
    type CommandOutput = InternalEvent;
    type Init = ();

    view! {
        // This is a non-visual component, no UI needed
        gtk::Box {
            set_visible: false,
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Self::new();
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match message {
            WhatsAppClientInput::Start { store_path } => {
                self.state = ClientState::Connecting;

                // Create event channel
                let (event_tx, mut event_rx) = mpsc::unbounded_channel::<WhatsAppClientEvent>();
                self.event_sender = Some(event_tx.clone());

                // Clone handles for async task
                let client_handle = self.client_handle.clone();

                // Spawn connection command
                sender.oneshot_command(async move {
                    // Forward events from channel to component
                    let sender_clone = sender.clone();
                    tokio::spawn(async move {
                        while let Some(event) = event_rx.recv().await {
                            let _ = sender_clone.output(event);
                        }
                    });

                    // Run client
                    match Self::run_client(store_path, event_tx, client_handle).await {
                        Ok(_) => InternalEvent::ConnectionResult(Ok(())),
                        Err(e) => InternalEvent::ConnectionResult(Err(e)),
                    }
                });
            }

            WhatsAppClientInput::Stop => {
                // TODO: Implement graceful shutdown
                self.state = ClientState::Disconnected;
            }

            WhatsAppClientInput::SendMessage { jid, text } => {
                let client_handle = self.client_handle.clone();
                sender.oneshot_command(async move {
                    match Self::send_message(client_handle, jid, text).await {
                        Ok(id) => InternalEvent::SendResult(Ok(id)),
                        Err(e) => InternalEvent::SendResult(Err(e)),
                    }
                });
            }

            WhatsAppClientInput::SendTyping { jid } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::send_typing(client_handle, jid, true).await;
                });
            }

            WhatsAppClientInput::StopTyping { jid } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::send_typing(client_handle, jid, false).await;
                });
            }

            WhatsAppClientInput::MarkRead { chat_jid, message_ids } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::mark_read(client_handle, chat_jid, message_ids).await;
                });
            }

            WhatsAppClientInput::AcceptCall { call_id } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::handle_call_action(client_handle, call_id, true).await;
                });
            }

            WhatsAppClientInput::DeclineCall { call_id } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::handle_call_action(client_handle, call_id, false).await;
                });
            }

            WhatsAppClientInput::StartCall { jid, is_video } => {
                let client_handle = self.client_handle.clone();
                sender.spawn_oneshot_command(async move {
                    let _ = Self::start_call(client_handle, jid, is_video).await;
                });
            }
        }
    }

    fn update_cmd(
        &mut self,
        message: Self::CommandOutput,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            InternalEvent::ClientEvent(event) => {
                // Convert and forward to output
                let ui_event = WhatsAppClientEvent::from(event);
                let _ = sender.output(ui_event);
            }

            InternalEvent::ConnectionResult(result) => {
                match result {
                    Ok(_) => {
                        self.state = ClientState::Connected;
                    }
                    Err(e) => {
                        self.state = ClientState::Error(e.clone());
                        let _ = sender.output(WhatsAppClientEvent::Error { message: e });
                    }
                }
            }

            InternalEvent::SendResult(result) => {
                match result {
                    Ok(id) => {
                        let _ = sender.output(WhatsAppClientEvent::MessageSent { id });
                    }
                    Err(e) => {
                        let _ = sender.output(WhatsAppClientEvent::Error { message: e });
                    }
                }
            }
        }
    }
}

// Additional helper methods for calls
impl WhatsAppClientComponent {
    async fn handle_call_action(
        client_handle: ClientHandle,
        call_id: String,
        accept: bool,
    ) -> Result<(), String> {
        let client = client_handle
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("Client not connected")?;

        // Note: Call handling would need to be implemented based on whatsapp-rust API
        // This is a placeholder for the call action handling
        if accept {
            // client.accept_call(&call_id).await...
        } else {
            // client.decline_call(&call_id).await...
        }

        Ok(())
    }

    async fn start_call(
        client_handle: ClientHandle,
        jid: String,
        is_video: bool,
    ) -> Result<(), String> {
        let client = client_handle
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or("Client not connected")?;

        let jid: wacore_binary::jid::Jid = jid.parse()
            .map_err(|e| format!("Invalid JID: {}", e))?;

        let options = CallOptions {
            video: is_video,
            ..Default::default()
        };

        // client.start_call(&jid, options).await...
        // Note: Actual call start API would depend on whatsapp-rust implementation

        Ok(())
    }
}

// Conversion from whatsapp-rust events to UI events
impl From<whatsapp_rust::types::events::Event> for WhatsAppClientEvent {
    fn from(event: whatsapp_rust::types::events::Event) -> Self {
        use whatsapp_rust::types::events::Event;

        match event {
            Event::PairingQrCode { code, .. } => {
                Self::QrCode {
                    url: format!("https://api.qrserver.com/v1/create-qr-code/?size=300x300&data={}", code),
                    code,
                }
            }
            Event::PairingCode { code, .. } => Self::PairCode { code },
            Event::Connected(_) => Self::Connected {
                phone_number: String::new(),
                pushname: String::new(),
            },
            Event::Message(msg, info) => Self::MessageReceived {
                message: Message { content: msg, info },
            },
            Event::Receipt(receipt) => Self::ReadReceipts {
                chat_jid: receipt.source.chat.to_string(),
                message_ids: receipt.message_ids.into_iter().map(|id| id.to_string()).collect(),
            },
            Event::CallOffer(offer) => Self::CallOffer {
                call_id: offer.meta.call_id,
                from_jid: offer.meta.from.to_string(),
                is_video: matches!(offer.media_type, whatsapp_rust::types::call::CallMediaType::Video),
            },
            Event::CallEnded(ended) => Self::CallEnded {
                call_id: ended.meta.call_id,
            },
            Event::Disconnected(_) => Self::Disconnected,
            Event::Error(err) => Self::Error {
                message: format!("{:?}", err),
            },
            _ => Self::Error {
                message: format!("Unhandled event type"),
            },
        }
    }
}

/// Wrapper for message data
#[derive(Debug, Clone)]
pub struct Message {
    pub content: std::sync::Arc<waproto::whatsapp::Message>,
    pub info: whatsapp_rust::types::message::MessageInfo,
}
