use std::{
    collections::{HashMap, HashSet, VecDeque},
    mem,
    time::Duration,
};

use adw::{NavigationSplitView, prelude::*};
use gtk::{gio, glib};
use indexmap::IndexMap;
use jiff::Timestamp;
use relm4::{
    abstractions::Toaster,
    actions::{AccelsPlus, RelmAction, RelmActionGroup},
    main_application,
    prelude::*,
};
use strum::{AsRefStr, EnumString};
use tokio::{task, time};
use uuid::Uuid;
use whatsapp_rust::{
    types::{message::MessageInfo, presence::ReceiptType},
    waproto::whatsapp::Message,
};

use crate::{
    components::{
        ChatList, ChatListInput, ChatListOutput, ChatView, ChatViewInput, ChatViewOutput, Login,
        LoginInput, LoginOutput, Welcome, WelcomeOutput,
    },
    config::{APP_ID, PROFILE},
    db::{
        entities::{Contact, Session},
        keyring::KeyringService,
        session::open_session_db,
        session_manager::SessionManager,
        store::SessionStore,
    },
    i18n, i18n_f,
    modals::{about::AboutDialog, shortcuts::ShortcutsDialog},
    session::{ChatsSyncedEntry, Client, ClientInput, ClientOutput, avatar},
    state::{Chat, ChatMessage, HistoryAnchor, Media, MediaType, MessageStatus, TypingSender},
    utils::{
        bare_jid, cmp_chat_order, format_lid_as_number, get_first_name, is_group_jid, is_lid_jid,
        is_phone_jid, is_status_broadcast, looks_like_jid,
    },
};

/// Seconds without history sync chunks after which the sync feedback hides.
const SYNC_QUIET_SECS: u64 = 30;

pub struct Application {
    db: SessionStore,
    page: AppPage,
    chats: Vec<Chat>,
    login: AsyncController<Login>,
    state: AppState,
    client: AsyncController<Client>,
    sender: AsyncComponentSender<Self>,
    typing: HashMap<String, ChatTypingState>,
    session: Session,
    /// Resolved contact list (JID -> name), real names only: entries
    /// seeded by the address book or the contacts store, never by
    /// wire push names.
    contacts: HashMap<String, String>,
    /// Wire push names (JID -> name), keyed bare. The freshest name
    /// seen on the wire wins; rendered `~name` italic in rows, never
    /// as a chat title.
    push_names: HashMap<String, String>,
    /// Whether chats have already been loaded from the database.
    cache_loaded: bool,
    /// LID siblings of the account's own jid (`user_jid` holds the
    /// phone form).
    own_lid_jids: Vec<String>,
    /// Bare jid of the chat currently shown in the chat view, if any;
    /// routes live name changes to the open rows.
    open_chat_jid: Option<String>,
    /// Whether the client is connected to `WhatsApp`.
    client_connected: bool,

    /// Pin mutations that arrived before the chat's row existed,
    /// replayed at populate and sync-chunk completion.
    pending_pins: Vec<(String, Option<i64>)>,
    /// Receipts that arrived before their message; applied when it lands.
    pending_receipts: HashMap<String, VecDeque<(String, MessageStatus, bool)>>,
    /// Read marks that arrived while the chats were not loaded yet.
    pending_read_chats: HashSet<String>,
    /// Avatar fetches queued while the client was not connected yet.
    pending_avatar_fetches: Vec<String>,

    toaster: Toaster,
    user_jid: Option<String>,
    welcome: AsyncController<Welcome>,
    chat_list: AsyncController<ChatList>,
    chat_view: AsyncController<ChatView>,
    split_view: NavigationSplitView,
    history_sync: HistorySyncStatus,
    session_page: AppSessionPage,
    user_push_name: Option<String>,
    synced_history_chats: HashSet<String>,
}

#[derive(Clone, Copy, Debug, AsRefStr, PartialEq, EnumString)]
#[strum(serialize_all = "lowercase")]
enum AppPage {
    Login,
    Session,
    Welcome,
    Error,
}

#[derive(Debug, PartialEq)]
enum AppState {
    Loading,

    Ready,
    Pairing,
    Syncing,
    Disconnected,

    Error(String),
}

#[derive(AsRefStr, Clone, Copy, Debug, EnumString, PartialEq)]
#[strum(serialize_all = "kebab-case")]
enum AppSessionPage {
    Empty,
    Syncing,
    ChatHistory,
}

struct HistorySyncStatus {
    active: bool,
    percent: Option<u32>,
    generation: u64,
}

#[derive(Debug)]
pub enum AppMsg {
    Connected {
        jid: Option<String>,
        push_name: String,
    },
    LoggedOut,
    ResetSession,
    Disconnected,
    SelfPushNameUpdated {
        push_name: String,
    },

    PairDevice {
        code: Option<String>,
        qr_code: Option<String>,
        timeout: Duration,
    },
    DevicePaired,
    PairWithPhoneNumber {
        phone_number: String,
    },
    SwitchToLoginQrCode,
    SwitchToLoginPhoneNumber,

    ChatOpen,
    ChatClosed,
    ChatSelected(String),
    MarkChatRead(String),

    AvatarUpdate {
        jid: String,
        path: String,
    },
    AvatarFailed {
        jid: String,
    },
    ContactUpdate {
        jid: String,
        name: Option<String>,
        push_name: Option<String>,
        phone_number: String,
    },
    ContactRemoved {
        jid: String,
    },
    ReceiptUpdate {
        chat_jid: String,
        message_ids: Vec<String>,
        receipt_type: ReceiptType,
    },
    PresenceUpdate {
        jid: String,
        available: bool,
        last_seen: Option<Timestamp>,
    },
    ChatPresenceUpdate {
        chat_jid: String,
        active: bool,
        recording: bool,
        sender_jid: String,
        sender_alt: Option<String>,
    },
    MessageStatusUpdate {
        chat_jid: String,
        msg_id: Uuid,
        status: MessageStatus,
    },

    LidPnResolved {
        chat_jid: String,
        lid: String,
        phone: Option<String>,
    },
    PushNamesLearned {
        chat_jid: Option<String>,
        entries: Vec<(String, String)>,
    },
    LidMappingsLearned {
        pairs: Vec<(String, String)>,
    },
    GroupParticipantsLearned {
        chat_jid: String,
        total: usize,
        entries: Vec<(String, String)>,
        participants: Vec<String>,
    },

    TypingExpired {
        chat_jid: String,
        sender_jid: String,
        generation: u64,
    },
    TypingStateChanged {
        chat_jid: String,
        composing: bool,
    },

    MessageReceived {
        info: Box<MessageInfo>,
        message: Box<Message>,
    },

    SendTextMessage {
        text: String,
        recipient: String,
    },

    ChatPinned {
        jid: String,
        at: Option<i64>,
    },
    ChatSubject {
        jid: String,
        subject: String,
    },
    ChatsSynced {
        entries: Vec<ChatsSyncedEntry>,
    },
    ChatReadOnDevice(String),
    ChatPropertyUpdate {
        jid: String,
        muted: Option<bool>,
        archived: Option<bool>,
    },

    FetchHistory {
        chat_jid: String,
        anchor: Option<HistoryAnchor>,
    },
    HistorySyncProgress {
        progress: Option<u32>,
        on_demand: bool,
    },
    HistorySyncCompleted,
    OfflineSyncCompleted,

    RequestSenderAvatar {
        jid: String,
    },

    Unknown,
    Error {
        message: String,
    },
    Quit,
}

#[derive(Debug)]
pub enum AppCmd {
    /// Sync cache from database.
    LoadCache,
    SwitchSession {
        db: SessionStore,
        session: Session,
    },

    SyncChats {
        entries: Vec<ChatsSyncedEntry>,
    },
    HistorySyncQuiet {
        generation: u64,
    },

    UpdateChat {
        chat: Chat,
    },
    AddChatToList {
        chat: Chat,
    },
    MarkedChatRead {
        chat: Chat,
        flipped: Vec<Uuid>,
    },
    ChatKeyRefreshed {
        jid: String,
        last_message_time: Timestamp,
    },
    HistoryBackfilled {
        chat_jid: String,
        has_messages: bool,
    },
}

#[derive(Debug, Default)]
struct ChatTypingState {
    senders: IndexMap<String, TypingSender>,
    generation: u64,
}

impl ChatTypingState {
    fn typing_senders(&self) -> Vec<TypingSender> {
        self.senders.values().cloned().collect()
    }
}

impl Application {
    fn add_chat(&mut self, mut chat: Chat) {
        let bare = bare_jid(&chat.jid);
        if self.is_own_jid(&bare) {
            chat.name = self.me_chat_title();
        } else if let Some(name) = self.contacts.get(&chat.jid).filter(|n| !n.is_empty()) {
            chat.name.clone_from(name);
        } else if is_phone_jid(&chat.jid)
            && chat.jid != self.user_jid.clone().unwrap_or_default()
            && format_lid_as_number(&chat.jid) != chat.name
        {
            chat.name = format_lid_as_number(&chat.jid);
        }

        self.chats.push(chat.clone());
        self.chats.sort_by(|a, b| {
            cmp_chat_order(
                &(a.pinned, a.pinned_at, a.last_message_time),
                &(b.pinned, b.pinned_at, b.last_message_time),
            )
        });

        let db = self.db.clone();
        let chat_clone = chat.clone();
        relm4::spawn(async move {
            if let Err(e) = chat_clone.upsert(&db).await {
                tracing::error!("Failed to save chat: {}", e);
            }
        });

        self.chat_list.emit(ChatListInput::AddChat { chat });
    }

    fn emit_typing(&self, chat_jid: &str) {
        let Some(state) = self.typing.get(chat_jid) else {
            return;
        };

        let senders = state.typing_senders();
        self.chat_list.emit(ChatListInput::UpdateTyping {
            chat_jid: chat_jid.to_string(),
            senders: senders.clone(),
        });

        self.chat_view.emit(ChatViewInput::TypingUpdate {
            chat_jid: chat_jid.to_string(),
            senders,
        });
    }

    fn register_participant(&mut self, chat_jid: &str, jid: &str, name: Option<&str>) {
        // The own participant renders as `You`, distinct from everyone
        // else however it is keyed.
        let bare = bare_jid(jid);
        if self.is_own_jid(&bare) {
            if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
                && chat.is_group()
            {
                chat.participants.insert(jid.to_string(), i18n!("You"));
            }

            return;
        }

        // Contacts always win as the display source; the wire name fills
        // the rest.
        let display = self
            .contacts
            .get(jid)
            .map(String::as_str)
            .or(name)
            .filter(|n| !n.is_empty());
        if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
            && chat.is_group()
            && let Some(display) = display
        {
            chat.participants
                .insert(jid.to_string(), display.to_string());
        }
    }

    /// Whether the jid is the account itself, in either identity form.
    fn is_own_jid(&self, bare: &str) -> bool {
        self.user_jid.as_deref().is_some_and(|own| own == bare)
            || self.own_lid_jids.iter().any(|lid| lid == bare)
    }

    /// The title of the account's own chat, so the me-chat row reads
    /// `+DD… (You)` instead of a bare number.
    fn me_chat_title(&self) -> String {
        self.user_jid.as_deref().map_or_else(
            || i18n!("You"),
            |own| format!("{} ({})", format_lid_as_number(own), i18n!("You")),
        )
    }

    /// The one name-resolution source: the direct contact entry first,
    /// then the contact seen under the jid's other identity (a LID
    /// mapped to its phone number or the other way around).
    async fn resolve_name(&self, jid: &str) -> Option<String> {
        self.lookup_name_in(&self.contacts, jid).await
    }

    /// Same sibling-aware lookup over the learned wire push names.
    async fn resolve_push_name(&self, jid: &str) -> Option<String> {
        self.lookup_name_in(&self.push_names, jid).await
    }

    async fn lookup_name_in(&self, map: &HashMap<String, String>, jid: &str) -> Option<String> {
        let bare = bare_jid(jid);

        if let Some(name) = map.get(&bare).filter(|n| !n.is_empty()) {
            return Some(name.clone());
        } else if is_lid_jid(&bare)
            && let Some(phone) = self.db.lid_to_pn_jid(&bare).await
            && let Some(name) = map.get(&phone).filter(|n| !n.is_empty())
        {
            return Some(name.clone());
        } else if is_phone_jid(&bare) {
            for lid in self.db.pn_to_lid_jids(&bare).await {
                let bare_lid = bare_jid(&lid);

                if let Some(name) = map.get(&bare_lid).filter(|n| !n.is_empty()) {
                    return Some(name.clone());
                }
            }
        }

        None
    }

    async fn resolve_sender_name(
        &self,
        jid: &str,
        alt_jid: Option<&str>,
        chat: Option<&Chat>,
        stored: Option<&str>,
    ) -> Option<String> {
        if let Some(name) = self.resolve_name(jid).await {
            return Some(name);
        } else if let Some(alt) = alt_jid
            && let Some(name) = self.resolve_name(alt).await
        {
            return Some(name);
        }

        // Everything past the real-name chain is wire-sourced: the
        // learned push name, a participants entry or the name this
        // message carries. Mark those the WhatsApp way.
        let bare = bare_jid(jid);
        let push_name = self
            .resolve_push_name(jid)
            .await
            .or_else(|| {
                chat.and_then(|c| c.participants.get(jid))
                    .filter(|n| **n != i18n!("Unknown"))
                    .cloned()
            })
            .or_else(|| {
                stored
                    .filter(|n| !n.is_empty())
                    .map(|n| n.trim_start_matches('~').to_string())
            });

        match push_name {
            Some(name) if !looks_like_jid(&name, &bare) => Some(format!("~{name}")),
            _ => self.resolve_sender_display(jid).await,
        }
    }

    /// Applies a pin mutation, matching the chat through either jid
    /// form so LID-keyed rows accept phone-keyed updates — the server
    /// redistributes pin events on every resync. Updates arriving
    /// before the chat's row exists buffer until the next chunk.
    async fn apply_pin(&mut self, jid: &str, at: Option<i64>) {
        let bare = bare_jid(jid);
        let mut forms = vec![bare.clone()];
        if let Some(pn) = self.db.lid_to_pn_jid(&bare).await {
            forms.push(bare_jid(&pn));
        }
        for lid in self.db.pn_to_lid_jids(&bare).await {
            forms.push(bare_jid(&lid));
        }

        let Some(chat) = self.chats.iter_mut().find(|c| forms.contains(&c.jid)) else {
            self.pending_pins.push((bare, at));
            return;
        };

        // The server replays pin state on every resync; skip the
        // save and the chat-list churn when nothing changed.
        if chat.pinned_at == at {
            return;
        }
        chat.pinned = at.is_some();
        chat.pinned_at = at;

        let db = self.db.clone();
        let to_save = chat.clone();
        relm4::spawn(async move {
            if let Err(e) = to_save.upsert(&db).await {
                tracing::error!("Failed to save pin state for {}: {e}", to_save.jid);
            }
        });

        self.chat_list
            .emit(ChatListInput::UpdateChat { chat: chat.clone() });
    }

    /// Nameless LID senders fall back to their phone number instead
    /// of opaque LID digits.
    async fn resolve_sender_display(&self, jid: &str) -> Option<String> {
        let bare = bare_jid(jid);
        if is_lid_jid(&bare)
            && let Some(pn) = self.db.lid_to_pn_jid(&bare).await
        {
            return Some(format_lid_as_number(&pn));
        } else if is_phone_jid(&bare) {
            return Some(format_lid_as_number(&bare));
        }

        None
    }

    /// Pushes a name change into the open chat so its rows update
    /// live instead of on the next open: resolves the sender's final
    /// display name and emits it with every jid identity the rows
    /// may carry them under.
    async fn refresh_open_chat_sender(&self, sender_jid: &str) {
        let Some(open_jid) = self.open_chat_jid.clone() else {
            return;
        };
        let Some(chat) = self.chats.iter().find(|c| c.jid == open_jid) else {
            return;
        };

        let bare = bare_jid(sender_jid);
        let name = self
            .resolve_sender_name(&bare, None, Some(chat), None)
            .await;

        let mut forms = vec![bare.clone()];
        if let Some(pn) = self.db.lid_to_pn_jid(&bare).await {
            forms.push(bare_jid(&pn));
        }
        for lid in self.db.pn_to_lid_jids(&bare).await {
            forms.push(bare_jid(&lid));
        }

        self.chat_view
            .emit(ChatViewInput::SenderNameChanged { forms, name });
    }

    /// Learns a contact's push name from a sync chunk or live message.
    async fn learn_push_name(&mut self, sender_jid: &str, push_name: &str) {
        if sender_jid.is_empty() {
            return;
        }

        let bare = bare_jid(sender_jid);
        if self
            .push_names
            .get(&bare)
            .is_some_and(|known| known == push_name)
        {
            return;
        }

        let contact = match self.db.get_contact(&bare).await {
            Ok(Some(contact)) => {
                let mut contact = contact;
                contact.push_name = Some(push_name.to_string());
                contact
            }
            Ok(None) => Contact {
                jid: bare.clone(),
                name: None,
                push_name: Some(push_name.to_string()),
                last_updated: 0,
                phone_number: None,
                is_registered: false,
                profile_picture_url: None,
            },
            Err(e) => {
                tracing::error!("Failed to load contact {bare}: {e}");
                return;
            }
        };

        if let Err(e) = self.db.save_contact(&contact).await {
            tracing::error!("Failed to save push name for {bare}: {e}");
            return;
        }

        self.push_names.insert(bare.clone(), push_name.to_string());
        self.refresh_open_chat_sender(&bare).await;
    }

    async fn resolve_jid(&self, jid: &str) -> String {
        let bare = bare_jid(jid);
        if is_lid_jid(&bare)
            && let Some(pn_jid) = self.db.lid_to_pn_jid(&bare).await
        {
            pn_jid
        } else {
            bare
        }
    }

    /// The phone may key a conversation by either jid form (LID or phone
    /// number). Attach it to the chat row papo knows, preferring the
    /// resolved canonical form so history lands where live messages do.
    async fn sync_target_jid(&self, jid: &str) -> String {
        let resolved = self.resolve_jid(jid).await;
        if self.chats.iter().any(|c| c.jid == resolved) {
            return resolved;
        }

        if self.chats.iter().any(|c| c.jid == jid) {
            return jid.to_string();
        }

        resolved
    }

    /// Applies a receipt to a stored message. Returns `true` when the status
    /// advanced. Receipts for unknown messages are buffered until the message
    /// arrives. `incoming_only` receipts (read/played on the phone) never
    /// touch outgoing messages.
    async fn apply_receipt(
        &mut self,
        chat: &Chat,
        chat_jid: &str,
        msg_id: String,
        status: MessageStatus,
        incoming_only: bool,
    ) -> bool {
        match chat.find_message(&self.db, &msg_id).await {
            Ok(Some(message)) => {
                if incoming_only && message.outgoing {
                    return false;
                }

                if status != MessageStatus::Failed && status.stage() <= message.status.stage() {
                    return false;
                }

                self.chat_view.emit(ChatViewInput::MessageStatusUpdate {
                    status,
                    local_id: message.local_id,
                });

                if let Err(e) = self.db.set_message_status(message.local_id, status).await {
                    tracing::error!("Failed to update message status: {}", e);
                }

                true
            }
            Ok(None) => {
                tracing::trace!("Receipt for unknown message {msg_id} (sync gap)");
                let buffer = self
                    .pending_receipts
                    .entry(chat_jid.to_string())
                    .or_default();
                if buffer.len() >= 64 {
                    buffer.remove(0);
                }

                buffer.push_back((msg_id, status, incoming_only));
                false
            }
            Err(e) => {
                tracing::warn!("Message {msg_id} not found: {e}");
                false
            }
        }
    }

    /// Applies receipts buffered while chats were not loaded yet. Entries for
    /// chats still unknown or messages still missing are kept.
    async fn flush_pending_receipts(&mut self) {
        let buffered = mem::take(&mut self.pending_receipts);

        for (chat_jid, receipts) in buffered {
            if let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned() {
                for (msg_id, status, incoming_only) in receipts {
                    self.apply_receipt(&chat, &chat_jid, msg_id, status, incoming_only)
                        .await;
                }

                self.chat_list.emit(ChatListInput::UpdateChat { chat });
            } else {
                self.pending_receipts.insert(chat_jid, receipts);
            }
        }
    }

    /// Marks a chat as read because the user read it on the phone. Chats
    /// that are not loaded yet are buffered until the cache loads.
    fn mark_read_on_device(&mut self, chat_jid: String) {
        if let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned() {
            let db = self.db.clone();
            let sender = self.sender.clone();
            sender.oneshot_command(async move {
                let flipped = chat
                    .mark_read(&db, false)
                    .await
                    .inspect_err(|e| tracing::error!("Failed to mark a chat as read: {e}"))
                    .unwrap_or_default();

                AppCmd::MarkedChatRead { chat, flipped }
            });
        } else {
            self.pending_read_chats.insert(chat_jid);
        }
    }

    /// The own chat is always read on the phone; offline echoes may have
    /// stamped `Sent` before the connection (and thus `user_jid`) existed.
    fn sweep_own_chat(&self) {
        let Some(user_jid) = self.user_jid.clone() else {
            return;
        };
        let Some(chat) = self.chats.iter().find(|chat| chat.jid == user_jid).cloned() else {
            return;
        };

        let db = self.db.clone();
        let sender = self.sender.clone();
        sender.oneshot_command(async move {
            let flipped = chat
                .mark_read(&db, true)
                .await
                .inspect_err(|e| {
                    tracing::error!("Failed to mark own chat as read: {e}");
                })
                .unwrap_or_default();

            AppCmd::MarkedChatRead { chat, flipped }
        });
    }

    async fn add_message(&mut self, chat_jid: &str, mut message: ChatMessage) {
        if !message.server_id.is_empty()
            && self
                .db
                .load_message_by_server_id(chat_jid, &message.server_id)
                .await
                .ok()
                .flatten()
                .is_some()
        {
            tracing::trace!("Skipping duplicate message: {}", message.server_id);
            return;
        }

        // Apply a buffered receipt that arrived ahead of the message.
        if !message.server_id.is_empty()
            && let Some(buffer) = self.pending_receipts.get_mut(chat_jid)
            && let Some(index) = buffer
                .iter()
                .position(|(id, _, _)| *id == message.server_id)
            && let Some((_, status, incoming_only)) = buffer.remove(index)
            && (!incoming_only || !message.outgoing)
            && (status == MessageStatus::Failed || status.stage() > message.status.stage())
        {
            message.status = status;
        }

        let is_group = is_group_jid(chat_jid);

        // Create a new chat if it doesn't exists.
        if !self.chats.iter().any(|c| c.jid == chat_jid) {
            let name = if is_group {
                format!("{} {}", i18n!("Group"), &chat_jid[..8])
            } else if self.user_jid.as_ref().is_some_and(|u_j| chat_jid == u_j) {
                i18n!("You")
            } else if let Some(name) = self.resolve_name(chat_jid).await {
                name
            } else {
                format_lid_as_number(chat_jid)
            };

            self.add_chat(Chat {
                jid: chat_jid.to_string(),
                name,
                muted: false,
                pinned: false,
                pinned_at: None,
                archived: false,
                available: None,
                last_seen: None,
                avatar_path: None,
                participants: HashMap::new(),
                member_count: None,
                last_message_time: message.timestamp,
            });

            self.client.emit(ClientInput::FetchAvatar {
                jid: chat_jid.to_string(),
            });
        }

        let typing_cleared = !message.outgoing
            && self
                .typing
                .get_mut(chat_jid)
                .is_some_and(|state| state.senders.shift_remove(&message.sender_jid).is_some());

        let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid) else {
            return;
        };

        if !message.outgoing && is_group && !chat.participants.contains_key(&message.sender_jid) {
            chat.participants.insert(
                message.sender_jid.clone(),
                message
                    .sender_name
                    .clone()
                    .unwrap_or_else(|| i18n!("Unknown")),
            );
        }

        // Save the chat and the message in the database.
        if let Err(e) = chat.upsert(&self.db).await {
            tracing::error!("Failed to update chat: {}", e);
        }
        if let Err(e) = message.upsert(&self.db).await {
            tracing::error!("Failed to save message: {}", e);
        }
        self.chat_view
            .emit(ChatViewInput::MessageReceived(Box::new(message)));

        let sender = self.sender.clone();
        let chat_clone = chat.clone();
        sender.oneshot_command(async move { AppCmd::UpdateChat { chat: chat_clone } });

        if typing_cleared {
            self.emit_typing(chat_jid);
        }
    }

    /// Fraction for the syncing progress bar, from the server percent or the
    /// count of chats that already received their history.
    fn sync_fraction(&self) -> Option<f64> {
        if !self.history_sync.active {
            return None;
        }

        if let Some(percent) = self.history_sync.percent {
            return Some(f64::from(percent) / 100.0);
        }

        let total = u32::try_from(self.chats.len()).ok()?;
        (total > 0).then(|| {
            let synced = u32::try_from(self.synced_history_chats.len()).unwrap_or(total);
            f64::from(synced) / f64::from(total)
        })
    }

    fn forward_sync_status(&self) {
        self.chat_view.emit(ChatViewInput::SyncProgress {
            active: self.history_sync.active,
            percent: self.history_sync.percent,
            synced: self.synced_history_chats.len(),
            total: self.chats.len(),
        });
    }

    /// Shows the sync feedback and restarts the quiet timeout that hides it
    /// once chunks stop arriving.
    fn activate_history_sync(&mut self, percent: Option<u32>, sender: &AsyncComponentSender<Self>) {
        self.history_sync.active = true;
        self.history_sync.percent = percent;
        self.history_sync.generation += 1;
        let generation = self.history_sync.generation;

        self.forward_sync_status();
        sender.oneshot_command(async move {
            time::sleep(Duration::from_secs(SYNC_QUIET_SECS)).await;
            AppCmd::HistorySyncQuiet { generation }
        });
    }

    fn deactivate_history_sync(&mut self) {
        self.history_sync.generation += 1;
        self.history_sync.active = false;
        self.history_sync.percent = None;
        self.forward_sync_status();
    }
}

relm4::new_action_group!(pub(super) WindowActionGroup, "win");
relm4::new_stateless_action!(ContactsAction, WindowActionGroup, "show-contacts");
relm4::new_stateless_action!(PreferencesAction, WindowActionGroup, "show-preferences");
relm4::new_stateless_action!(pub(super) ShortcutsAction, WindowActionGroup, "show-help-overlay");
relm4::new_stateless_action!(AboutAction, WindowActionGroup, "about");
relm4::new_stateless_action!(QuitAction, WindowActionGroup, "quit");

#[relm4::component(async, pub)]
impl AsyncComponent for Application {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Widgets = AppWidgets;
    type CommandOutput = AppCmd;

    menu! {
        primary_menu: {
            section! {
                &i18n!("_Contacts") => ContactsAction,
            },
            section! {
                &i18n!("_Preferences") => PreferencesAction,
                &i18n!("_Keyboard Shortcuts") => ShortcutsAction,
                &i18n!("_About Papo") => AboutAction,
            }
        }
    }

    view! {
        #[root]
        main_window = adw::ApplicationWindow::new(&main_application()) {
            set_title: Some(&i18n!("Papo")),
            set_visible: true,
            set_width_request: 360,
            set_height_request: 440,
            set_default_width: 900,
            set_default_height: 850,

            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::Quit);
                glib::Propagation::Stop
            },

            add_css_class?: (PROFILE == "Devel").then_some("devel"),

            #[local_ref]
            toast_overlay -> adw::ToastOverlay {
                #[name = "main_stack"]
                gtk::Stack {
                    set_transition_type: gtk::StackTransitionType::Crossfade,

                    #[local_ref]
                    add_named[Some("welcome")] = welcome_widget -> adw::ToolbarView {},

                    #[local_ref]
                    add_named[Some("login")] = login_widget -> adw::ToolbarView {},

                    add_named[Some("session")] = &adw::BreakpointBin {
                        set_width_request: main_window.width_request(),
                        set_height_request: main_window.height_request(),

                        #[local_ref]
                        #[wrap(Some)]
                        set_child = split_view -> adw::NavigationSplitView {
                            set_min_sidebar_width: 280.0,
                            set_max_sidebar_width: 360.0,

                            #[name = "sidebar"]
                            #[wrap(Some)]
                            set_sidebar = &adw::NavigationPage {
                                set_title: &i18n!("Papo"),
                                set_css_classes: &["background"],

                                #[wrap(Some)]
                                set_child = &adw::ToolbarView {
                                    add_top_bar = &adw::HeaderBar {
                                        set_show_title: false,

                                        pack_start = &gtk::ToggleButton {
                                            set_css_classes: &["flat", "circular"],
                                            set_tooltip_text: Some(&i18n!("Your profile")),

                                            adw::Avatar {
                                                #[watch]
                                                set_text: Some(&model.user_push_name.clone().unwrap_or_else(|| i18n!("You"))),
                                                set_size: 30,
                                                set_show_initials: true,
                                            }
                                        },
                                        pack_end = &gtk::MenuButton {
                                            set_icon_name: "menu-symbolic",
                                            set_menu_model: Some(&primary_menu),
                                            set_tooltip_text: Some(&i18n!("Menu")),
                                        },
                                    },

                                    #[name = "view_stack"]
                                    #[wrap(Some)]
                                    set_content = &adw::ViewStack {
                                        #[local_ref]
                                        add_titled[Some("chats"), &i18n!("Chats")] = chat_list_widget -> gtk::Box {} -> {
                                            set_icon_name: Some("chat-bubbles-text-symbolic")
                                        },

                                        /* add_titled[Some("status"), &i18n!("Status")] = &gtk::ScrolledWindow {} -> {
                                            set_icon_name: Some("image-round-symbolic")
                                        } */
                                    },

                                    add_bottom_bar = &adw::ViewSwitcherBar {
                                        set_stack: Some(&view_stack),
                                        set_reveal: true
                                    },
                                },
                            },

                            #[name = "content"]
                            #[wrap(Some)]
                            set_content = &adw::NavigationPage {
                                set_title: &i18n!("Chat"),
                                set_css_classes: &["view"],

                                #[wrap(Some)]
                                set_child = &gtk::Stack {
                                    set_transition_type: gtk::StackTransitionType::Crossfade,

                                    add_named[Some("empty")] = &adw::StatusPage {
                                        set_title: &i18n!("No Chat Selected"),
                                        set_hexpand: true,
                                        set_vexpand: true,
                                        set_can_focus: false,
                                        set_icon_name: Some("chat-bubbles-empty-symbolic"),
                                        set_description: Some(&i18n!("Select a chat to start chatting"))
                                    },

                                    add_named[Some("syncing")] = &gtk::Box {
                                        set_halign: gtk::Align::Center,
                                        set_valign: gtk::Align::Center,
                                        set_spacing: 12,
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_width_request: 360,

                                        gtk::Image {
                                            set_icon_name: Some("arrows-bidirectional-symbolic"),
                                            set_pixel_size: 96,
                                        },

                                        gtk::Label {
                                            set_label: &i18n!("Syncing History"),
                                            set_css_classes: &["title-2"],
                                        },

                                        gtk::Label {
                                            set_wrap: true,
                                            set_label: &i18n!("This may take a few minutes..."),
                                            set_justify: gtk::Justification::Center,
                                            set_css_classes: &["dimmed"],
                                        },

                                        gtk::Label {
                                            #[watch]
                                            set_label: i18n_f!(
                                                "{0}%",
                                                model.history_sync.percent.unwrap_or_default()
                                            )
                                            .as_str(),
                                            #[watch]
                                            set_visible: model.history_sync.percent.is_some(),
                                            set_css_classes: &["title-4"],
                                        },

                                        #[name = "syncing_progress_bar"]
                                        gtk::ProgressBar {
                                            set_hexpand: true,
                                            #[watch]
                                            set_fraction?: model.sync_fraction(),
                                            set_css_classes: &["syncing-progress"],
                                        },

                                        gtk::Label {
                                            #[watch]
                                            set_label: i18n_f!(
                                                "Synced {0} of {1} chats",
                                                model.synced_history_chats.len(),
                                                model.chats.len()
                                            )
                                            .as_str(),
                                            #[watch]
                                            set_visible: !model.chats.is_empty(),
                                            set_css_classes: &["dimmed"],
                                        },
                                    },

                                    #[local_ref]
                                    add_named[Some("chat-history")] = chat_view_widget -> adw::ToolbarView {},

                                    #[watch]
                                    set_visible_child_name: syncing_content_page(
                                        model.history_sync.active,
                                        model.session_page
                                    )
                                    .as_ref(),
                                }
                            }
                        },

                        add_breakpoint = bp_with_setters(
                            adw::Breakpoint::new(
                                adw::BreakpointCondition::new_length(
                                    adw::BreakpointConditionLengthType::MaxWidth,
                                    600.0,
                                    adw::LengthUnit::Sp,
                                )
                            ),
                            &[(split_view, "collapsed", true)]
                        ),
                    },

                    #[watch]
                    set_visible_child_name: model.page.as_ref(),
                },
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: AsyncComponentSender<Self>,
    ) -> AsyncComponentParts<Self> {
        let keyring = KeyringService::new()
            .await
            .expect("Failed to access keyring");
        let mut session_manager = SessionManager::new(keyring)
            .await
            .expect("Failed to open main database");
        let (session, has_existing_session) = match session_manager.last_valid_session().await {
            Ok(Some(session)) => (session, true),
            _ => (
                session_manager
                    .create_session()
                    .await
                    .expect("Failed to create session"),
                false,
            ),
        };

        let db = open_session_db(&session.uuid, session_manager.keyring())
            .await
            .expect("Failed to open session database");
        let db = SessionStore::new(db, &session.uuid);

        let login =
            Login::builder()
                .launch(())
                .forward(sender.input_sender(), |output| match output {
                    LoginOutput::ResetSession => AppMsg::ResetSession,

                    LoginOutput::PairWithPhoneNumber { phone_number } => {
                        AppMsg::PairWithPhoneNumber { phone_number }
                    }
                });

        let mut contacts = HashMap::new();
        let mut push_names = HashMap::new();
        if let Ok(stored) = db.get_all_contacts().await {
            for contact in stored {
                let Contact {
                    jid,
                    name,
                    push_name,
                    ..
                } = contact;
                if let Some(name) = name.filter(|n| !n.is_empty()) {
                    contacts.insert(jid, name);
                } else if let Some(push_name) = push_name.filter(|n| !n.is_empty()) {
                    push_names.insert(jid, push_name);
                }
            }
        }

        let client =
            Client::builder()
                .launch(db.clone())
                .forward(sender.input_sender(), |output| match output {
                    ClientOutput::Connected { jid, push_name } => {
                        AppMsg::Connected { jid, push_name }
                    }
                    ClientOutput::LoggedOut => AppMsg::LoggedOut,
                    ClientOutput::Disconnected => AppMsg::Disconnected,
                    ClientOutput::SelfPushNameUpdated { push_name } => {
                        AppMsg::SelfPushNameUpdated { push_name }
                    }

                    ClientOutput::PairCode {
                        code,
                        qr_code,
                        timeout,
                    } => AppMsg::PairDevice {
                        code,
                        qr_code,
                        timeout,
                    },
                    ClientOutput::PairSuccess => AppMsg::DevicePaired,

                    ClientOutput::ReceiptUpdate {
                        chat_jid,
                        message_ids,
                        receipt_type,
                    } => AppMsg::ReceiptUpdate {
                        chat_jid,
                        message_ids,
                        receipt_type,
                    },
                    ClientOutput::PresenceUpdate {
                        jid,
                        available,
                        last_seen,
                    } => AppMsg::PresenceUpdate {
                        jid,
                        available,
                        last_seen,
                    },
                    ClientOutput::ChatPresenceUpdate {
                        chat_jid,
                        active,
                        recording,
                        sender_jid,
                        sender_alt,
                    } => AppMsg::ChatPresenceUpdate {
                        chat_jid,
                        active,
                        recording,
                        sender_jid,
                        sender_alt,
                    },

                    ClientOutput::MessageReceived { info, message } => {
                        AppMsg::MessageReceived { info, message }
                    }
                    ClientOutput::MessageSent { chat_jid, msg_id } => AppMsg::MessageStatusUpdate {
                        chat_jid,
                        msg_id,
                        status: MessageStatus::Sent,
                    },
                    ClientOutput::MessageFailed { chat_jid, msg_id } => {
                        AppMsg::MessageStatusUpdate {
                            chat_jid,
                            msg_id,
                            status: MessageStatus::Failed,
                        }
                    }

                    ClientOutput::ChatPinned { jid, at } => AppMsg::ChatPinned { jid, at },
                    ClientOutput::ChatSubject { chat_jid, subject } => AppMsg::ChatSubject {
                        jid: chat_jid,
                        subject,
                    },
                    ClientOutput::ChatsSynced { entries } => AppMsg::ChatsSynced { entries },
                    ClientOutput::ChatReadOnDevice { jid } => AppMsg::ChatReadOnDevice(jid),
                    ClientOutput::ChatPropertyUpdate {
                        jid,
                        muted,
                        archived,
                    } => AppMsg::ChatPropertyUpdate {
                        jid,
                        muted,
                        archived,
                    },

                    ClientOutput::HistorySyncProgress {
                        progress,
                        on_demand,
                    } => AppMsg::HistorySyncProgress {
                        progress,
                        on_demand,
                    },
                    ClientOutput::HistorySyncCompleted => AppMsg::HistorySyncCompleted,
                    ClientOutput::OfflineSyncCompleted => AppMsg::OfflineSyncCompleted,

                    ClientOutput::AvatarUpdate { jid, path } => AppMsg::AvatarUpdate { jid, path },
                    ClientOutput::AvatarFailed { jid } => AppMsg::AvatarFailed { jid },
                    ClientOutput::ContactUpdate {
                        jid,
                        name,
                        push_name,
                        phone_number,
                    } => AppMsg::ContactUpdate {
                        jid,
                        name,
                        push_name,
                        phone_number,
                    },
                    ClientOutput::ContactRemoved { jid } => AppMsg::ContactRemoved { jid },

                    ClientOutput::LidPnResolved {
                        chat_jid,
                        lid,
                        phone,
                    } => AppMsg::LidPnResolved {
                        chat_jid,
                        lid,
                        phone,
                    },
                    ClientOutput::PushNameLearned { entries } => AppMsg::PushNamesLearned {
                        chat_jid: None,
                        entries,
                    },
                    ClientOutput::LidMappingsLearned { pairs } => {
                        AppMsg::LidMappingsLearned { pairs }
                    }
                    ClientOutput::GroupParticipantsLearned {
                        chat_jid,
                        total,
                        entries,
                        participants,
                    } => AppMsg::GroupParticipantsLearned {
                        chat_jid,
                        total,
                        entries,
                        participants,
                    },

                    ClientOutput::Error { message } => AppMsg::Error { message },
                    _ => AppMsg::Unknown,
                });

        let welcome = Welcome::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                WelcomeOutput::PairWithQrCode => AppMsg::SwitchToLoginQrCode,
                WelcomeOutput::PairWithPhoneNumber => AppMsg::SwitchToLoginPhoneNumber,
            });
        let chat_list =
            ChatList::builder()
                .launch(db.clone())
                .forward(sender.input_sender(), |output| match output {
                    ChatListOutput::ChatSelected(jid) => AppMsg::ChatSelected(jid),
                });
        let chat_view =
            ChatView::builder()
                .launch(db.clone())
                .forward(sender.input_sender(), |output| match output {
                    ChatViewOutput::ChatOpen => AppMsg::ChatOpen,
                    ChatViewOutput::ChatClosed => AppMsg::ChatClosed,
                    ChatViewOutput::MarkChatRead(jid) => AppMsg::MarkChatRead(jid),

                    ChatViewOutput::TypingStateChanged {
                        chat_jid,
                        composing,
                    } => AppMsg::TypingStateChanged {
                        chat_jid,
                        composing,
                    },

                    ChatViewOutput::SendTextMessage { text, recipient } => {
                        AppMsg::SendTextMessage { text, recipient }
                    }

                    ChatViewOutput::FetchHistory { chat_jid, anchor } => {
                        AppMsg::FetchHistory { chat_jid, anchor }
                    }

                    ChatViewOutput::RequestSenderAvatar { jid } => {
                        AppMsg::RequestSenderAvatar { jid }
                    }
                });

        let model = Self {
            db,
            page: if has_existing_session {
                AppPage::Session
            } else {
                AppPage::Welcome
            },
            chats: Vec::new(),
            login,
            state: AppState::Loading,
            client,
            sender: sender.clone(),
            typing: HashMap::new(),
            session,
            contacts,
            push_names,
            cache_loaded: false,
            own_lid_jids: Vec::new(),
            open_chat_jid: None,
            client_connected: false,

            pending_pins: Vec::new(),
            pending_receipts: HashMap::new(),
            pending_read_chats: HashSet::new(),
            pending_avatar_fetches: Vec::new(),

            toaster: Toaster::default(),
            user_jid: None,
            welcome,
            chat_list,
            chat_view,
            split_view: NavigationSplitView::new(),
            history_sync: HistorySyncStatus {
                active: false,
                percent: None,
                generation: 0,
            },
            session_page: AppSessionPage::Empty,
            user_push_name: None,
            synced_history_chats: HashSet::new(),
        };

        let split_view = &model.split_view;
        let login_widget = model.login.widget();
        let welcome_widget = model.welcome.widget();
        let toast_overlay = model.toaster.overlay_widget();
        let chat_list_widget = model.chat_list.widget();
        let chat_view_widget = model.chat_view.widget();

        let app = root.application().unwrap();
        let mut actions = RelmActionGroup::<WindowActionGroup>::new();

        let shortcuts_action = {
            RelmAction::<ShortcutsAction>::new_stateless(move |_| {
                ShortcutsDialog::builder().launch(()).detach();
            })
        };

        let about_action = {
            RelmAction::<AboutAction>::new_stateless(move |_| {
                AboutDialog::builder().launch(()).detach();
            })
        };

        let quit_action = {
            let sender = sender.clone();
            RelmAction::<QuitAction>::new_stateless(move |_| {
                sender.input(AppMsg::Quit);
            })
        };

        // Connect actions with hotkeys
        app.set_accelerators_for_action::<QuitAction>(&["<Control>q"]);
        // app.set_accelerators_for_action::<QuitAction>(&["<Control>w"]);

        let widgets = view_output!();

        // Pulse the syncing progress bar while its fraction is unknown.
        let sync_bar = widgets.syncing_progress_bar.clone();
        glib::timeout_add_local(Duration::from_millis(400), move || {
            if sync_bar.is_visible() && sync_bar.fraction() == 0.0 {
                sync_bar.pulse();
            }
            glib::ControlFlow::Continue
        });

        // Mirror libadwaita's own back navigation gesture as a chat close.
        let notify_sender = sender.clone();
        split_view.connect_notify(Some("show-content"), move |split_view, _| {
            if !split_view.shows_content() {
                notify_sender.input(AppMsg::ChatClosed);
            }
        });

        actions.add_action(shortcuts_action);
        actions.add_action(about_action);
        actions.add_action(quit_action);
        actions.register_for_widget(&widgets.main_window);

        widgets.load_window_size();

        // Load the session while the client connects in background.
        if has_existing_session {
            sender.oneshot_command(async { AppCmd::LoadCache });
        }

        AsyncComponentParts { model, widgets }
    }

    #[allow(clippy::too_many_lines)]
    async fn update(
        &mut self,
        message: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            AppMsg::Connected { jid, push_name } => {
                self.user_jid = jid.map(|jid| bare_jid(&jid));
                self.user_push_name = Some(push_name);
                self.client_connected = true;

                // Participants keyed by the account's LID forms are the
                // account itself.
                if let Some(own) = self.user_jid.clone() {
                    self.own_lid_jids = self
                        .db
                        .pn_to_lid_jids(&own)
                        .await
                        .into_iter()
                        .map(|lid| bare_jid(&lid))
                        .collect();
                }

                // Chats from the database already show for returning users;
                // this is the first load after a fresh pairing.
                if !self.cache_loaded {
                    sender.oneshot_command(async { AppCmd::LoadCache });
                }

                if self.page != AppPage::Session {
                    self.page = AppPage::Session;
                }

                for jid in mem::take(&mut self.pending_avatar_fetches) {
                    self.client.emit(ClientInput::FetchAvatar { jid });
                }

                self.sweep_own_chat();
            }

            AppMsg::LoggedOut => {
                self.page = AppPage::Welcome;
                self.state = AppState::Pairing;
                self.chats.clear();
                self.synced_history_chats.clear();
                self.pending_read_chats.clear();
                self.deactivate_history_sync();

                let session_uuid = self.session.uuid.clone();
                sender.oneshot_command(async move {
                    let keyring = KeyringService::new()
                        .await
                        .expect("Failed to access keyring");
                    let mut session_manager = SessionManager::new(keyring)
                        .await
                        .expect("Failed to open main database");
                    session_manager
                        .delete_session(&session_uuid)
                        .await
                        .expect("Failed to delete session");

                    let session = session_manager
                        .create_session()
                        .await
                        .expect("Failed to create session");
                    let db = open_session_db(&session.uuid, session_manager.keyring())
                        .await
                        .expect("Failed to open session database");

                    AppCmd::SwitchSession {
                        db: SessionStore::new(db, &session.uuid),
                        session,
                    }
                });
            }
            AppMsg::Disconnected => {
                self.state = AppState::Disconnected;
                self.client_connected = false;
            }
            AppMsg::SelfPushNameUpdated { push_name } => {
                self.user_push_name = Some(push_name);
            }
            AppMsg::ResetSession => {
                self.client.emit(ClientInput::Restart);
            }

            AppMsg::PairDevice {
                code,
                qr_code,
                timeout,
            } => {
                self.page = AppPage::Welcome;
                self.login.emit(LoginInput::PairCode {
                    code,
                    qr_code,
                    timeout,
                });
            }
            AppMsg::DevicePaired => {
                self.login.emit(LoginInput::PairSuccess);
                time::sleep(Duration::from_secs(2)).await;

                self.page = AppPage::Session;
                self.state = AppState::Syncing;
                self.synced_history_chats.clear();
                self.pending_read_chats.clear();
                self.activate_history_sync(None, &sender);
            }
            AppMsg::PairWithPhoneNumber { phone_number } => {
                self.client
                    .emit(ClientInput::PairWithPhoneNumber { phone_number });
            }

            AppMsg::SwitchToLoginQrCode => {
                self.page = AppPage::Login;
                self.login.emit(LoginInput::PairWithQrCode);
            }
            AppMsg::SwitchToLoginPhoneNumber => {
                self.page = AppPage::Login;
                self.login
                    .emit(LoginInput::PairWithPhoneNumber { edit: false });
            }

            AppMsg::ChatOpen => {
                self.split_view.set_show_content(true);
                self.session_page = AppSessionPage::ChatHistory;
            }
            AppMsg::ChatClosed => {
                self.open_chat_jid = None;
                self.chat_list.emit(ChatListInput::ClearSelection);
                self.split_view.set_show_content(false);
                self.session_page = AppSessionPage::Empty;
            }
            AppMsg::ChatSelected(jid) => {
                self.open_chat_jid = Some(jid.clone());
                if let Some(chat) = self.chats.iter().find(|c| c.jid == jid).cloned() {
                    self.chat_view.emit(ChatViewInput::Open(chat.clone()));

                    if chat.is_group() {
                        self.client
                            .emit(ClientInput::FetchGroupMetadata { chat_jid: chat.jid });
                    }
                }

                if let Some(state) = self.typing.get(&jid)
                    && !state.senders.is_empty()
                {
                    self.emit_typing(&jid);
                }
            }
            AppMsg::MarkChatRead(jid) => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    // The user's own chat receives their outgoing messages.
                    let own_chat = self.user_jid.as_ref().is_some_and(|u| u == &jid);
                    let messages = chat.get_unread_messages(&self.db).await.unwrap_or_default();

                    // Separate messages by sender.
                    let mut sender_messages = IndexMap::<String, Vec<String>>::new();
                    for message in messages {
                        let sender_jid = message.sender_jid;

                        sender_messages
                            .entry(sender_jid)
                            .or_default()
                            .push(message.server_id);
                    }

                    // Send read receipts to WhatsApp.
                    for (sender_jid, message_ids) in sender_messages {
                        self.client.emit(ClientInput::MarkRead {
                            chat_jid: jid.clone(),
                            sender_jid: Some(sender_jid),
                            message_ids,
                        });
                    }

                    // Mark chat as read locally, then update the chat list.
                    let db = self.db.clone();
                    let sender = self.sender.clone();
                    let chat_clone = chat.clone();
                    sender.oneshot_command(async move {
                        let flipped = chat_clone
                            .mark_read(&db, own_chat)
                            .await
                            .inspect_err(|e| tracing::error!("Failed to mark a chat as read: {e}"))
                            .unwrap_or_default();

                        AppCmd::MarkedChatRead {
                            chat: chat_clone,
                            flipped,
                        }
                    });
                }
            }

            AppMsg::AvatarUpdate { jid, path } => {
                let bare = bare_jid(&jid);
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    chat.avatar_path = Some(path.clone());

                    self.chat_list.emit(ChatListInput::UpdateAvatar {
                        jid,
                        avatar_path: path,
                    });

                    tracing::trace!("Updated avatar for chat: {}", chat.jid);
                }

                self.chat_view
                    .emit(ChatViewInput::SenderAvatarUpdated { jid: bare });
            }
            AppMsg::AvatarFailed { jid } => {
                self.chat_view
                    .emit(ChatViewInput::SenderAvatarRetry { jid });
            }
            AppMsg::ContactUpdate {
                mut jid,
                name,
                push_name,
                phone_number,
            } => {
                jid = bare_jid(&jid);
                if let Some(name) = name.clone().filter(|n| !n.is_empty()) {
                    self.contacts.insert(jid.clone(), name);
                    self.push_names.remove(&jid);
                }
                if let Some(push_name) = push_name.clone().filter(|n| !n.is_empty()) {
                    self.push_names.insert(jid.clone(), push_name);
                }

                // The open chat may show this contact; re-render its
                // rows with the new name live.
                self.refresh_open_chat_sender(&jid).await;

                // Save contact to database in background.
                let db = self.db.clone();
                let jid_for_contact = jid.clone();
                let name_for_contact = name.clone();

                let contact = Contact {
                    jid: jid.clone(),
                    name: name.clone(),
                    push_name,
                    last_updated: 0,
                    phone_number: Some(phone_number),
                    is_registered: true,
                    profile_picture_url: None,
                };

                relm4::spawn(async move {
                    if let Err(e) = db.save_contact(&contact).await {
                        tracing::error!("Failed to save contact {}: {}", jid_for_contact, e);
                    } else {
                        tracing::debug!(
                            "Saved contact: {} (name: {:?})",
                            jid_for_contact,
                            name_for_contact
                        );
                    }
                });

                let mut forms = vec![jid.clone()];
                if is_lid_jid(&jid)
                    && let Some(pn_jid) = self.db.lid_to_pn_jid(&jid).await
                {
                    forms.push(pn_jid);
                } else if is_phone_jid(&jid) {
                    forms.extend(self.db.pn_to_lid_jids(&jid).await);
                }

                // Update chat name if this contact has a chat and we got a name.
                if let Some(contact_name) = self.contacts.get(&jid) {
                    if let Some(chat) = self.chats.iter_mut().find(|c| forms.contains(&c.jid)) {
                        chat.name.clone_from(contact_name);

                        let db = self.db.clone();
                        let jid_clone = jid.clone();
                        let chat_clone = chat.clone();
                        relm4::spawn(async move {
                            if let Err(e) = chat_clone.upsert(&db).await {
                                tracing::error!(
                                    "Failed to update chat name for {}: {}",
                                    jid_clone,
                                    e
                                );
                            }
                        });

                        self.chat_list
                            .emit(ChatListInput::UpdateChat { chat: chat.clone() });

                        // The open chat's header renders from its own
                        // clone; hand it the fresh metadata.
                        if self.open_chat_jid.as_ref() == Some(&chat.jid) {
                            self.chat_view
                                .emit(ChatViewInput::ChatUpdated { chat: chat.clone() });
                        }

                        tracing::info!("Updated chat name for {} to: {}", jid, contact_name);
                    }

                    for chat in &mut self.chats {
                        let mut touched = false;
                        for form in &forms {
                            if let Some(entry) = chat.participants.get_mut(form) {
                                entry.clone_from(contact_name);
                                touched = true;
                            }
                        }

                        if touched {
                            self.chat_list
                                .emit(ChatListInput::UpdateChat { chat: chat.clone() });
                        }
                    }
                }
            }
            AppMsg::ContactRemoved { jid } => {
                let removed_name = self.contacts.remove(&jid);
                self.refresh_open_chat_sender(&jid).await;

                // If the removed contact titled a chat, fall that title
                // back to the phone number everywhere, including the
                // open chat's header.
                if let Some(removed_name) = removed_name {
                    let bare = bare_jid(&jid);
                    let mut forms = vec![bare.clone()];
                    if is_lid_jid(&bare)
                        && let Some(pn_jid) = self.db.lid_to_pn_jid(&bare).await
                    {
                        forms.push(pn_jid);
                    } else if is_phone_jid(&bare) {
                        forms.extend(self.db.pn_to_lid_jids(&bare).await);
                    }

                    if let Some(chat) = self
                        .chats
                        .iter_mut()
                        .find(|c| forms.contains(&c.jid) && c.name == removed_name)
                    {
                        chat.name = String::new();

                        let db = self.db.clone();
                        let chat_clone = chat.clone();
                        relm4::spawn(async move {
                            if let Err(e) = chat_clone.upsert(&db).await {
                                tracing::error!(
                                    "Failed to update chat name for {}: {}",
                                    chat_clone.jid,
                                    e
                                );
                            }
                        });

                        self.chat_list
                            .emit(ChatListInput::UpdateChat { chat: chat.clone() });

                        if self.open_chat_jid.as_ref() == Some(&chat.jid) {
                            self.chat_view
                                .emit(ChatViewInput::ChatUpdated { chat: chat.clone() });
                        }
                    }
                }
            }
            AppMsg::ReceiptUpdate {
                mut chat_jid,
                message_ids,
                receipt_type,
            } => {
                chat_jid = self.resolve_jid(&chat_jid).await;
                let own_chat = self.user_jid.as_ref().is_some_and(|u| *u == chat_jid);

                // `ReadSelf`/`PlayedSelf` mean the user read the message on
                // the phone. In the own chat (outgoing messages) that marks
                // the delivery status; elsewhere it only clears the unread
                // state of incoming messages.
                let (status, incoming_only) = match receipt_type {
                    ReceiptType::ReadSelf if own_chat => (Some(MessageStatus::Read), false),
                    ReceiptType::PlayedSelf if own_chat => (Some(MessageStatus::Played), false),
                    ReceiptType::ReadSelf => (Some(MessageStatus::Read), true),
                    ReceiptType::PlayedSelf => (Some(MessageStatus::Played), true),
                    receipt_type => (MessageStatus::try_from(receipt_type).ok(), false),
                };

                if let Some(status) = status
                    && let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned()
                {
                    for msg_id in message_ids {
                        self.apply_receipt(&chat, &chat_jid, msg_id, status, incoming_only)
                            .await;
                    }

                    self.chat_list.emit(ChatListInput::UpdateChat { chat });
                } else if let Some(status) = status {
                    // Chats are not loaded yet (offline replay at connect);
                    // the flush after loading applies these.
                    let buffer = self.pending_receipts.entry(chat_jid).or_default();
                    for msg_id in message_ids {
                        if buffer.len() >= 64 {
                            buffer.remove(0);
                        }

                        buffer.push_back((msg_id, status, incoming_only));
                    }
                }
            }
            AppMsg::PresenceUpdate {
                jid,
                available,
                last_seen,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    if !chat.is_group() {
                        chat.available = Some(available);
                    }

                    chat.last_seen = last_seen;
                }

                self.chat_view.emit(ChatViewInput::PresenceUpdate {
                    jid,
                    available,
                    last_seen,
                });
            }
            AppMsg::ChatPresenceUpdate {
                chat_jid,
                active,
                recording,
                sender_jid,
                sender_alt,
            } => {
                let name = self.chats.iter().find(|c| c.jid == chat_jid).map(|chat| {
                    let resolved = if chat.is_group() {
                        chat.participants
                            .get(&sender_jid)
                            .or_else(|| {
                                sender_alt
                                    .as_ref()
                                    .and_then(|alt| chat.participants.get(alt))
                            })
                            .filter(|name| !name.is_empty() && **name != i18n!("Unknown"))
                            .map_or_else(
                                || {
                                    let sender_jid = sender_jid.as_str();
                                    let contact = self
                                        .contacts
                                        .get(sender_jid)
                                        .or_else(|| {
                                            sender_alt
                                                .as_ref()
                                                .and_then(|alt| self.contacts.get(alt.as_str()))
                                        })
                                        .filter(|name| !name.is_empty());

                                    if let Some(contact) = contact {
                                        return contact.clone();
                                    }

                                    let phone = if is_lid_jid(sender_jid) {
                                        sender_alt.as_deref().filter(|alt| !is_lid_jid(alt))
                                    } else {
                                        Some(sender_jid)
                                    };
                                    if phone.is_none() {
                                        self.client.emit(ClientInput::ResolveLidPn {
                                            chat_jid: chat_jid.clone(),
                                            lid: sender_jid.to_string(),
                                        });
                                    }

                                    phone.map_or_else(|| i18n!("Someone"), format_lid_as_number)
                                },
                                String::clone,
                            )
                    } else {
                        chat.name.trim().to_string()
                    };

                    if resolved.starts_with('+') {
                        resolved
                    } else {
                        get_first_name(&resolved)
                    }
                });

                if active && let Some(name) = name {
                    let state = self.typing.entry(chat_jid.clone()).or_default();
                    state.generation += 1;
                    let generation = state.generation;

                    state.senders.insert(
                        sender_jid.clone(),
                        TypingSender {
                            jid: sender_jid.clone(),
                            name,
                            recording,
                        },
                    );
                    self.emit_typing(&chat_jid);

                    let sender = sender.clone();
                    relm4::spawn(async move {
                        time::sleep(Duration::from_secs(10)).await;
                        sender.input(AppMsg::TypingExpired {
                            chat_jid,
                            sender_jid,
                            generation,
                        });
                    });
                } else {
                    let removed = self
                        .typing
                        .get_mut(&chat_jid)
                        .is_some_and(|state| state.senders.shift_remove(&sender_jid).is_some());
                    if removed {
                        self.emit_typing(&chat_jid);
                    }
                }
            }
            AppMsg::MessageStatusUpdate {
                chat_jid,
                msg_id,
                status,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
                    && let Ok(Some(mut message)) =
                        chat.find_message_by_local_id(&self.db, &msg_id).await
                    && (status == MessageStatus::Failed || status.stage() > message.status.stage())
                {
                    message.status = status;
                    self.chat_view.emit(ChatViewInput::MessageStatusUpdate {
                        status: message.status,
                        local_id: message.local_id,
                    });

                    // Update the message in the database, then update the chat
                    // list.
                    if let Err(e) = self.db.set_message_status(msg_id, status).await {
                        tracing::error!("Failed to update message status: {}", e);
                    }

                    self.chat_list
                        .emit(ChatListInput::UpdateChat { chat: chat.clone() });
                }
            }

            AppMsg::LidPnResolved {
                chat_jid,
                lid,
                phone,
            } => {
                if let Some(phone) = phone
                    && is_group_jid(&chat_jid)
                {
                    let pn_jid = format!("{phone}@s.whatsapp.net");
                    let name = self
                        .contacts
                        .get(&pn_jid)
                        .cloned()
                        .filter(|n| !n.is_empty());

                    self.register_participant(&chat_jid, &lid, name.as_deref());
                    self.register_participant(&chat_jid, &pn_jid, name.as_deref());

                    let resolved = name
                        .clone()
                        .unwrap_or_else(|| format_lid_as_number(&pn_jid));
                    if let Some(name) = &name {
                        // Alias the real name under the LID form so later
                        // lookups resolve; a phone number never poses as
                        // a contact name.
                        self.contacts.insert(lid.clone(), name.clone());
                    }

                    if let Some(state) = self.typing.get(&chat_jid)
                        && let Some(recording) = state.senders.get(&lid).map(|s| s.recording)
                    {
                        let display = if resolved.starts_with('+') {
                            resolved
                        } else {
                            get_first_name(&resolved)
                        };
                        if let Some(state) = self.typing.get_mut(&chat_jid) {
                            state.senders.insert(
                                lid.clone(),
                                TypingSender {
                                    jid: lid,
                                    name: display,
                                    recording,
                                },
                            );
                        }

                        self.emit_typing(&chat_jid);
                    }
                }
            }
            // Type-4 chunk entries (no chat context) ride the persisting
            // learner; metadata batches arrive persisted, spawned off-queue.
            AppMsg::PushNamesLearned { chat_jid, entries } => {
                for (jid, name) in entries {
                    let bare = bare_jid(&jid);
                    if self.user_jid.as_ref().is_some_and(|user| *user == bare) {
                        continue;
                    }

                    match &chat_jid {
                        Some(chat_jid) => {
                            self.push_names.insert(bare.clone(), name.clone());
                            self.register_participant(chat_jid, &jid, Some(&name));
                            self.refresh_open_chat_sender(&bare).await;
                        }
                        None => self.learn_push_name(&jid, &name).await,
                    }
                }

                // Registered participants fill the chat header
                // subtitle.
                if let Some(chat_jid) = chat_jid
                    && is_group_jid(&chat_jid)
                    && self.open_chat_jid.as_ref() == Some(&chat_jid)
                    && let Some(chat) = self.chats.iter().find(|c| c.jid == chat_jid).cloned()
                {
                    self.chat_view.emit(ChatViewInput::ChatUpdated { chat });
                }
            }
            AppMsg::LidMappingsLearned { pairs } => {
                for (lid, phone) in pairs {
                    if let Err(e) = self.db.learn_lid_mapping(&lid, &phone).await {
                        tracing::error!("Failed to learn LID mapping {lid} -> {phone}: {e}");
                    }
                }
            }
            AppMsg::GroupParticipantsLearned {
                chat_jid,
                total,
                mut entries,
                participants,
            } => {
                let open = self.open_chat_jid.as_ref();
                let updated_open_chat = self
                    .chats
                    .iter_mut()
                    .find(|c| c.jid == chat_jid.as_str())
                    .and_then(|chat| {
                        chat.member_count = (total > 0).then_some(total);
                        (open == Some(&chat.jid)).then(|| chat.clone())
                    });

                // Participant jids the server left nameless still name
                // people we know: resolve each against the contacts map.
                if !self.contacts.is_empty() {
                    for jid in &participants {
                        let bare = bare_jid(jid);
                        if bare == chat_jid {
                            continue;
                        }

                        // The account reads as `You` in the subtitle even
                        // though it never resolves to a contact.
                        if self.is_own_jid(&bare) {
                            self.register_participant(&chat_jid, &bare, None);
                            continue;
                        }

                        if let Some(name) = self.lookup_name_in(&self.contacts, &bare).await {
                            entries.push((bare, name));
                        }
                    }
                }

                let fresh = entries
                    .iter()
                    .filter(|(jid, name)| {
                        self.push_names.get(jid).map(String::as_str) != Some(name.as_str())
                    })
                    .cloned()
                    .collect::<Vec<(String, String)>>();

                let db = self.db.clone();
                let sender = sender.clone();
                let user_jid = self.user_jid.clone();
                relm4::spawn(async move {
                    for (jid, name) in fresh {
                        let bare = bare_jid(&jid);
                        if user_jid.as_ref().is_some_and(|u| *u == bare) {
                            continue;
                        }

                        match db.get_contact(&bare).await {
                            Ok(Some(mut contact)) => {
                                if contact.push_name.as_deref() == Some(name.as_str()) {
                                    continue;
                                }

                                contact.push_name = Some(name.clone());
                                if let Err(e) = db.save_contact(&contact).await {
                                    tracing::debug!("Failed to save contact {bare}: {e}");
                                }
                            }
                            Ok(None) => {
                                let contact = Contact {
                                    jid: bare.clone(),
                                    name: None,
                                    push_name: Some(name.clone()),
                                    last_updated: 0,
                                    phone_number: None,
                                    is_registered: false,
                                    profile_picture_url: None,
                                };
                                if let Err(e) = db.save_contact(&contact).await {
                                    tracing::debug!("Failed to save contact {bare}: {e}");
                                }
                            }
                            Err(e) => {
                                tracing::debug!("Failed to load contact {bare}: {e}");
                            }
                        }
                    }

                    sender.input(AppMsg::PushNamesLearned {
                        chat_jid: Some(chat_jid),
                        entries,
                    });
                });

                if let Some(chat) = updated_open_chat {
                    self.chat_view.emit(ChatViewInput::ChatUpdated { chat });
                }
            }

            AppMsg::TypingExpired {
                chat_jid,
                sender_jid,
                generation,
            } => {
                let removed = self.typing.get_mut(&chat_jid).is_some_and(|state| {
                    state.generation == generation
                        && state.senders.shift_remove(&sender_jid).is_some()
                });
                if removed {
                    self.emit_typing(&chat_jid);
                }
            }
            AppMsg::TypingStateChanged {
                chat_jid,
                composing,
            } => {
                if composing {
                    self.client.emit(ClientInput::SendTyping { jid: chat_jid });
                } else {
                    self.client.emit(ClientInput::StopTyping { jid: chat_jid });
                }
            }

            AppMsg::MessageReceived { info, mut message } => {
                if let Some(mut sent) = message.device_sent_message.take()
                    && let Some(inner) = sent.message.take()
                {
                    *message = inner;
                }

                let media = Media::from_wa_message(&message);
                let content = message
                    .conversation
                    .clone()
                    .filter(|c| !c.is_empty())
                    .or_else(|| {
                        message
                            .extended_text_message
                            .as_option()
                            .and_then(|e| e.text.clone().filter(|t| !t.is_empty()))
                    })
                    .or_else(|| media.as_ref().and_then(|m| m.caption.clone()));

                if content.is_some() || media.is_some() {
                    if is_status_broadcast(&info.source.chat.to_string()) {
                        // TODO: handle status events
                    } else {
                        let chat_jid = self.resolve_jid(&info.source.chat.to_string()).await;
                        let outgoing = info.source.is_from_me;

                        let sender_jid = if outgoing {
                            self.user_jid.clone().unwrap_or_default()
                        } else if let Some(alt) = info
                            .source
                            .sender_alt
                            .as_ref()
                            .filter(|alt| !is_lid_jid(&alt.to_string()))
                        {
                            alt.to_string()
                        } else {
                            self.resolve_jid(&info.source.sender.to_string()).await
                        };

                        // Learn the wire push name before resolving.
                        if !outgoing && !info.push_name.is_empty() {
                            self.learn_push_name(&sender_jid, info.push_name.as_str())
                                .await;
                        }

                        let sender_name = self
                            .resolve_sender_name(
                                &sender_jid,
                                Some(&info.source.sender.to_string()),
                                self.chats.iter().find(|c| c.jid == chat_jid),
                                Some(info.push_name.as_str()),
                            )
                            .await;

                        let own_chat = self.user_jid.as_ref().is_some_and(|u| *u == chat_jid);
                        let status = if outgoing && own_chat {
                            MessageStatus::Read
                        } else {
                            MessageStatus::Sent
                        };

                        let mut chat_message = ChatMessage {
                            local_id: Uuid::new_v4(),
                            server_id: info.id.to_string(),
                            chat_jid: chat_jid.clone(),
                            sender_jid: sender_jid.clone(),
                            sender_name,

                            media,
                            quote: ChatMessage::wire_quote(&message),
                            status,
                            content: content.unwrap_or_default(),
                            outgoing,
                            reactions: IndexMap::new(),
                            timestamp: Timestamp::from_second(info.timestamp.timestamp())
                                .expect("Invalid timestamp"),
                        };

                        if let Some(quote) = chat_message.quote.as_mut()
                            && let Some(participant) = quote.participant.clone()
                        {
                            let chat = self.chats.iter().find(|chat| chat.jid == chat_jid).cloned();
                            let mut sender_name = self
                                .resolve_sender_name(&participant, None, chat.as_ref(), None)
                                .await;
                            if sender_name.is_none() {
                                sender_name = self.resolve_sender_display(&participant).await;
                            }
                            quote.sender_name = sender_name;
                        }
                        self.add_message(&chat_jid, chat_message).await;

                        if !outgoing {
                            let raw_sender = info.source.sender.to_string();
                            let name =
                                (!info.push_name.is_empty()).then_some(info.push_name.as_str());
                            self.register_participant(&chat_jid, &sender_jid, name);
                            if raw_sender != sender_jid {
                                self.register_participant(&chat_jid, &raw_sender, name);
                            }

                            let alt_jid = info.source.sender_alt.as_ref().map(ToString::to_string);
                            if let Some(alt_jid) = alt_jid.as_ref() {
                                self.register_participant(&chat_jid, alt_jid, name);
                            }

                            let removed = self.typing.get_mut(&chat_jid).is_some_and(|state| {
                                state.senders.shift_remove(&sender_jid).is_some()
                                    || state.senders.shift_remove(&raw_sender).is_some()
                                    || alt_jid.as_ref().is_some_and(|alt| {
                                        state.senders.shift_remove(alt).is_some()
                                    })
                            });
                            if removed {
                                self.emit_typing(&chat_jid);
                            }
                        }
                    }
                } else {
                    tracing::trace!(
                        "Message without content received: info = {:#?}, message = {:#?}",
                        info,
                        message
                    );
                }
            }

            AppMsg::SendTextMessage { text, recipient } => {
                if let Some(chat) = self.chats.iter().find(|c| c.jid == recipient).cloned() {
                    let timestamp = Timestamp::now();
                    let message = ChatMessage {
                        local_id: Uuid::new_v4(),
                        server_id: String::new(), // will be replaced later by the client.
                        chat_jid: recipient,
                        sender_jid: self.user_jid.clone().unwrap_or_default(),
                        sender_name: self.user_push_name.clone(),

                        media: None,
                        quote: None,
                        status: MessageStatus::Sending,
                        content: text,
                        outgoing: true,
                        reactions: IndexMap::new(),
                        timestamp,
                    };

                    // Save the message in the database.
                    let db = self.db.clone();
                    let msg_clone = message.clone();
                    relm4::spawn(async move {
                        if let Err(e) = msg_clone.upsert(&db).await {
                            tracing::error!("Failed to save message: {}", e);
                        }
                    });

                    self.client.emit(ClientInput::SendMessage {
                        message: Box::new(message.clone()),
                    });
                    self.chat_view
                        .emit(ChatViewInput::MessageReceived(Box::new(message)));
                    self.chat_list.emit(ChatListInput::UpdateChat { chat });
                }
            }

            AppMsg::ChatPinned { jid, at } => {
                self.apply_pin(&jid, at).await;
            }
            AppMsg::ChatSubject { jid, subject } => {
                let bare = bare_jid(&jid);
                let mut forms = vec![bare.clone()];
                if let Some(pn) = self.db.lid_to_pn_jid(&bare).await {
                    forms.push(bare_jid(&pn));
                }
                for lid in self.db.pn_to_lid_jids(&bare).await {
                    forms.push(bare_jid(&lid));
                }

                if let Some(existing) = self.chats.iter_mut().find(|c| forms.contains(&c.jid))
                    && existing.name != subject
                {
                    existing.name.clone_from(&subject);

                    let db = self.db.clone();
                    let to_save = existing.clone();
                    relm4::spawn(async move {
                        if let Err(e) = to_save.upsert(&db).await {
                            tracing::error!(
                                "Failed to save group subject for {}: {e}",
                                to_save.jid
                            );
                        }
                    });

                    self.chat_list.emit(ChatListInput::UpdateChat {
                        chat: existing.clone(),
                    });

                    if self.open_chat_jid.as_ref() == Some(&existing.jid) {
                        self.chat_view.emit(ChatViewInput::ChatUpdated {
                            chat: existing.clone(),
                        });
                    }
                }
            }
            AppMsg::ChatsSynced { entries } => {
                sender.oneshot_command(async move { AppCmd::SyncChats { entries } });
            }
            AppMsg::ChatReadOnDevice(jid) => {
                let chat_jid = self.resolve_jid(&jid).await;
                self.mark_read_on_device(chat_jid);
            }
            AppMsg::ChatPropertyUpdate {
                jid,
                muted,
                archived,
            } => {
                if let Some(chat) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    if let Some(muted) = muted {
                        chat.muted = muted;
                    }
                    if let Some(archived) = archived {
                        chat.archived = archived;
                    }

                    // Always save the chat to the database (including archive state).
                    let db = self.db.clone();
                    let chat_clone = chat.clone();
                    relm4::spawn(async move {
                        if let Err(e) = chat_clone.upsert(&db).await {
                            tracing::error!("Failed to save chat property update: {}", e);
                        }
                    });

                    if let Some(archived) = archived {
                        if archived {
                            self.chat_list
                                .emit(ChatListInput::RemoveChat { jid: jid.clone() });
                        } else {
                            self.chat_list
                                .emit(ChatListInput::AddChat { chat: chat.clone() });
                        }
                    } else {
                        self.chat_list
                            .emit(ChatListInput::UpdateChat { chat: chat.clone() });
                    }
                }
            }

            AppMsg::FetchHistory { chat_jid, anchor } => {
                let chat_jid = self.resolve_jid(&chat_jid).await;

                // The client component paces the actual requests: the
                // phone drops bursts, so papo queues them one at a time.
                self.client
                    .emit(ClientInput::FetchHistory { chat_jid, anchor });
            }
            AppMsg::HistorySyncProgress {
                progress,
                on_demand,
            } => {
                if !on_demand {
                    self.activate_history_sync(progress, &sender);
                } else if self.history_sync.active {
                    self.forward_sync_status();
                }
            }
            AppMsg::HistorySyncCompleted => {
                tracing::info!("History sync completed");
                if self.state == AppState::Syncing {
                    self.state = AppState::Ready;
                }
            }
            AppMsg::OfflineSyncCompleted => {
                tracing::info!("Offline sync completed");
                if self.state == AppState::Syncing {
                    self.state = AppState::Ready;
                }
            }

            AppMsg::RequestSenderAvatar { jid } => {
                if self
                    .user_jid
                    .as_ref()
                    .is_some_and(|user| *user != bare_jid(&jid))
                {
                    self.client.emit(ClientInput::FetchAvatar { jid });
                }
            }

            AppMsg::Unknown => {}
            AppMsg::Error { message } => {
                self.state = AppState::Error(message.clone());

                match self.page {
                    AppPage::Login => {
                        self.login.emit(LoginInput::Error { message });
                    }
                    AppPage::Session | AppPage::Welcome | AppPage::Error => {
                        // TODO: display error
                    }
                }
            }
            AppMsg::Quit => main_application().quit(),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn update_cmd(
        &mut self,
        command: Self::CommandOutput,
        _sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match command {
            AppCmd::LoadCache => {
                self.state = AppState::Syncing;
                let mut chats_needing_avatars = Vec::new();

                match self.db.load_chats().await {
                    Ok(mut chats) => {
                        tracing::info!("Loaded {} chats from own database", chats.len());

                        // Check for existing cached avatars.
                        let avatar_cache = avatar::global();
                        for chat in &mut chats {
                            let bare = bare_jid(&chat.jid);
                            if self.is_own_jid(&bare) {
                                chat.name = self.me_chat_title();
                            } else if let Some(name) =
                                self.contacts.get(&chat.jid).filter(|n| !n.is_empty())
                            {
                                chat.name.clone_from(name);
                            } else if is_phone_jid(&chat.jid)
                                && format_lid_as_number(&chat.jid) != chat.name
                            {
                                chat.name = format_lid_as_number(&chat.jid);
                            }

                            match avatar_cache {
                                Some(cache) if !cache.is_stale(&chat.jid) => {
                                    chat.avatar_path = Some(
                                        cache.avatar_path(&chat.jid).to_string_lossy().into_owned(),
                                    );
                                }
                                _ => chats_needing_avatars.push(chat.jid.clone()),
                            }
                        }

                        // The shared status conversation never appears as
                        // a chat row; its events are dropped on arrival.
                        chats.retain(|chat| !is_status_broadcast(&chat.jid));

                        self.chats.extend(chats.clone());
                        self.chat_list.emit(ChatListInput::PopulateAll(chats));
                        self.flush_pending_receipts().await;

                        // Read marks buffered while chats were not loaded.
                        let pending_read = mem::take(&mut self.pending_read_chats);
                        for chat_jid in pending_read {
                            self.mark_read_on_device(chat_jid);
                        }

                        // Pin mutations buffered before the rows existed.
                        let pins = mem::take(&mut self.pending_pins);
                        for (jid, at) in pins {
                            self.apply_pin(&jid, at).await;
                        }

                        self.cache_loaded = true;
                        self.sweep_own_chat();
                    }
                    Err(e) => tracing::error!("Failed to load chats from own database: {}", e),
                }

                self.state = AppState::Ready;
                if self.client_connected {
                    for jid in chats_needing_avatars {
                        self.client.emit(ClientInput::FetchAvatar { jid });
                    }
                } else {
                    // The client is still connecting; fetch once it is up.
                    self.pending_avatar_fetches.extend(chats_needing_avatars);
                }
            }
            AppCmd::SwitchSession { db, session } => {
                self.client
                    .emit(ClientInput::NewSession { store: db.clone() });
                self.db = db;
                self.session = session;
            }

            AppCmd::SyncChats { entries } => {
                // New spine chats ride one batch to the list per chunk;
                // one-per-entry inputs would flood its queue with DB
                // reads and starve clicks during the storm.
                let mut spine_new = Vec::new();

                for entry in entries {
                    let ChatsSyncedEntry {
                        jid,
                        name,
                        pinned,
                        pinned_at,
                        archived,
                        messages,
                        on_demand,
                        unread_count,
                        participants,
                        last_message_time,
                        ..
                    } = entry;
                    let jid = self.sync_target_jid(&jid).await;
                    let chat_jid = jid.clone();

                    // Status conversations carry no chat history in papo.
                    if is_status_broadcast(&jid) {
                        continue;
                    }

                    if !messages.is_empty() {
                        self.synced_history_chats.insert(chat_jid.clone());
                    }

                    // The phone's own unread state for the chat: zero
                    // means the user read it there, which no receipt will
                    // ever announce after the fact. A backfill chunk landing
                    // on the OPEN chat is also read: the user is watching
                    // it fill (opening = read), and stale Delivered
                    // statuses on incoming history would paint month-old
                    // pages unread until phone receipts heal them.
                    let resolved = self.resolve_jid(&chat_jid).await;
                    let open_chat = self
                        .open_chat_jid
                        .as_ref()
                        .is_some_and(|open| *open == chat_jid || *open == resolved);
                    if unread_count.is_some_and(|count| count == 0) || open_chat {
                        self.mark_read_on_device(chat_jid.clone());
                    }

                    let is_new_chat = !self.chats.iter().any(|c| c.jid == jid);
                    if is_new_chat {
                        self.client.emit(ClientInput::FetchAvatar {
                            jid: chat_jid.clone(),
                        });

                        // A 1:1 sync name is the peer's wire push name:
                        // learn it, never title the chat from it.
                        let own_chat = self.user_jid.as_ref().is_some_and(|u_j| jid == *u_j);
                        if !is_group_jid(&jid)
                            && !own_chat
                            && let Some(wire_name) = name.clone().filter(|n| !n.is_empty())
                        {
                            self.learn_push_name(&jid, &wire_name).await;
                        }

                        let mut chat_name = if is_group_jid(&jid) { name } else { None }
                            .or_else(|| self.contacts.get(&jid).cloned());
                        if chat_name.is_none() && !is_group_jid(&jid) {
                            chat_name = self.resolve_name(&jid).await;
                        }

                        let chat_name = chat_name.unwrap_or_else(|| {
                            if is_group_jid(&jid) {
                                format!("{} {}", i18n!("Group"), &jid[..8.min(jid.len())])
                            } else if own_chat {
                                i18n!("You")
                            } else {
                                format_lid_as_number(&jid)
                            }
                        });

                        // Create last message time from timestamp (already in seconds).
                        let last_message_time = last_message_time
                            .and_then(|ts| Timestamp::from_second(ts.cast_signed()).ok())
                            .unwrap_or(Timestamp::MIN);

                        // Create participants map for groups.
                        let mut participants_map = HashMap::new();
                        for (pjid, pname) in participants {
                            participants_map
                                .insert(pjid, pname.unwrap_or_else(|| i18n!("Unknown")));
                        }

                        let chat = Chat {
                            jid,
                            name: chat_name,
                            muted: false, // TODO: handle mute_end_time
                            pinned,
                            pinned_at,
                            archived,
                            available: None,
                            last_seen: None,
                            avatar_path: None,
                            participants: participants_map,
                            member_count: None,
                            last_message_time,
                        };

                        self.chats.push(chat.clone());
                        self.chats.sort_by(|a, b| {
                            cmp_chat_order(
                                &(a.pinned, a.pinned_at, a.last_message_time),
                                &(b.pinned, b.pinned_at, b.last_message_time),
                            )
                        });
                        spine_new.push(chat.clone());

                        // Save the chat to database.
                        let db = self.db.clone();
                        relm4::spawn(async move {
                            if let Err(e) = chat.upsert(&db).await {
                                tracing::error!("Failed to save synced chat {}: {}", chat.jid, e);
                            } else {
                                tracing::debug!(
                                    "Synced chat from history: {} (archived: {}, pinned: {})",
                                    chat.jid,
                                    archived,
                                    pinned
                                );
                            }
                        });
                    }

                    let db = self.db.clone();
                    let is_group = is_group_jid(&chat_jid);

                    // Learn push names carried by the chunk.
                    let mut learned = IndexMap::new();
                    for message in &messages {
                        if let Some(name) = message
                            .sender_name
                            .as_deref()
                            .filter(|name| !name.is_empty())
                            .filter(|_| !message.outgoing)
                        {
                            learned.insert(bare_jid(&message.sender_jid), name.to_string());
                        }
                    }
                    for (bare, push_name) in learned {
                        self.learn_push_name(&bare, &push_name).await;
                        task::yield_now().await;
                    }

                    // Collect sender info for participant updates.
                    let sender_info: Vec<(String, Option<String>)> = if is_group {
                        messages
                            .iter()
                            .filter(|m| !m.outgoing && !m.sender_jid.is_empty())
                            .map(|m| (m.sender_jid.clone(), m.sender_name.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    };

                    // Update participants for groups immediately.
                    if is_group
                        && let Some(chat) = self.chats.iter_mut().find(|c| c.jid == chat_jid)
                    {
                        for (sender_jid, sender_name) in &sender_info {
                            let entry = chat
                                .participants
                                .entry(sender_jid.clone())
                                .or_insert_with(|| i18n!("Unknown"));
                            if let Some(name) = sender_name.as_deref().filter(|n| !n.is_empty())
                                && *entry == i18n!("Unknown")
                            {
                                *entry = name.to_string();
                            }
                        }
                    }

                    let chat_ref = self.chats.iter().find(|c| c.jid == chat_jid);
                    let mut resolved_names = Vec::with_capacity(messages.len());
                    for message in &messages {
                        resolved_names.push(
                            self.resolve_sender_name(
                                &message.sender_jid,
                                None,
                                chat_ref,
                                message.sender_name.as_deref(),
                            )
                            .await,
                        );

                        task::yield_now().await;
                    }

                    // Spawn database operations in background task.
                    let chat_clone = chat_ref.cloned();
                    let sender = self.sender.clone();
                    relm4::spawn(async move {
                        let mut saved_count = 0;
                        let mut dup_count = 0;
                        let mut skip_count = 0;
                        let total = messages.len();
                        let has_messages = total > 0;

                        // Newest wire timestamp of the chunk: the chat's
                        // fresh sort key once the saves complete.
                        let newest = messages
                            .iter()
                            .map(|m| m.timestamp)
                            .max()
                            .and_then(|s| Timestamp::from_second(s.cast_signed()).ok());

                        for (synced_msg, sender_name) in messages.into_iter().zip(resolved_names) {
                            task::yield_now().await;

                            let media = synced_msg.media_type.map(|t| Media {
                                r#type: MediaType::from(t),
                                ..Media::default()
                            });
                            let content = synced_msg.content;

                            // Skip messages without content for now.
                            if content.is_none() && media.is_none() {
                                skip_count += 1;
                                continue;
                            }

                            // Timestamp is already in seconds (Unix timestamp).
                            let timestamp =
                                Timestamp::from_second(synced_msg.timestamp.cast_signed())
                                    .unwrap_or(Timestamp::MIN);

                            let message = ChatMessage {
                                media,
                                quote: synced_msg.quote,
                                status: synced_msg.status,
                                content: content.unwrap_or_default(),
                                chat_jid: chat_jid.clone(),
                                local_id: Uuid::new_v4(),
                                outgoing: synced_msg.outgoing,
                                reactions: IndexMap::new(),
                                sender_jid: synced_msg.sender_jid.clone(),
                                server_id: synced_msg.id,
                                timestamp,
                                sender_name,
                            };

                            // Save the message, skipping duplicates on server_id.
                            match message.save_or_ignore(&db).await {
                                Ok(true) => saved_count += 1,
                                Ok(false) => dup_count += 1,
                                Err(e) => tracing::error!("Failed to save synced message: {}", e),
                            }
                        }

                        tracing::debug!(
                            "Synced {} messages for chat: {} (of {} received, {} duplicates, {} without content)",
                            saved_count,
                            chat_jid,
                            total,
                            dup_count,
                            skip_count
                        );

                        if let Some(mut chat) = chat_clone {
                            if let Some(ts) = newest {
                                chat.last_message_time = chat.last_message_time.max(ts);
                            }

                            if is_new_chat && !archived {
                                sender
                                    .oneshot_command(async move { AppCmd::AddChatToList { chat } });
                            } else {
                                sender.oneshot_command(async move {
                                    AppCmd::ChatKeyRefreshed {
                                        jid: chat.jid,
                                        last_message_time: chat.last_message_time,
                                    }
                                });
                            }
                        }

                        // Tell an open chat view that its history landed,
                        // only once the messages are durably saved.
                        if has_messages || on_demand {
                            sender.oneshot_command(async move {
                                AppCmd::HistoryBackfilled {
                                    chat_jid,
                                    has_messages,
                                }
                            });
                        }
                    });
                }

                // The chunk's new chats, in display order, become rows in
                // one background read; the pump inserts each at its
                // sorted position.
                if !spine_new.is_empty() {
                    spine_new.sort_by(|a, b| {
                        cmp_chat_order(
                            &(a.pinned, a.pinned_at, a.last_message_time),
                            &(b.pinned, b.pinned_at, b.last_message_time),
                        )
                    });
                    self.chat_list.emit(ChatListInput::PopulateAll(spine_new));
                }

                // Pin mutations buffered before the chunk's rows existed
                // replay now; still-missing ones re-buffer.
                let pins = mem::take(&mut self.pending_pins);
                for (jid, at) in pins {
                    self.apply_pin(&jid, at).await;
                }

                if self.history_sync.active {
                    self.forward_sync_status();
                }
            }
            AppCmd::HistorySyncQuiet { generation } => {
                if generation == self.history_sync.generation && self.history_sync.active {
                    tracing::debug!("History sync quiet timeout reached");
                    self.history_sync.active = false;
                    self.history_sync.percent = None;
                    self.forward_sync_status();
                }
            }

            AppCmd::UpdateChat { chat } => {
                if let Some(existing) = self.chats.iter_mut().find(|c| c.jid == chat.jid) {
                    *existing = chat.clone();
                }

                self.chat_list.emit(ChatListInput::UpdateChat { chat });
            }
            AppCmd::AddChatToList { chat } => {
                self.chat_list.emit(ChatListInput::AddChat { chat });
            }
            AppCmd::MarkedChatRead { chat, flipped } => {
                for local_id in flipped {
                    self.chat_view.emit(ChatViewInput::MessageStatusUpdate {
                        status: MessageStatus::Read,
                        local_id,
                    });
                }

                self.chat_list.emit(ChatListInput::UpdateChat { chat });
            }
            AppCmd::ChatKeyRefreshed {
                jid,
                last_message_time,
            } => {
                if let Some(existing) = self.chats.iter_mut().find(|c| c.jid == jid) {
                    existing.last_message_time = last_message_time;

                    self.chat_list.emit(ChatListInput::UpdateChat {
                        chat: existing.clone(),
                    });
                }
            }
            AppCmd::HistoryBackfilled {
                chat_jid,
                has_messages,
            } => {
                self.chat_view.emit(ChatViewInput::HistoryBackfilled {
                    chat_jid,
                    has_messages,
                });
            }
        }
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        widgets.save_window_size().unwrap();
    }
}

impl AppWidgets {
    fn load_window_size(&self) {
        let settings = gio::Settings::new(APP_ID);

        let width = settings.int("window-width");
        let height = settings.int("window-height");
        let is_maximized = settings.boolean("is-maximized");

        self.main_window.set_default_size(width, height);

        if is_maximized {
            self.main_window.maximize();
        }
    }

    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let settings = gio::Settings::new(APP_ID);
        let (width, height) = self.main_window.default_size();

        settings.set_int("window-width", width)?;
        settings.set_int("window-height", height)?;

        settings.set_boolean("is-maximized", self.main_window.is_maximized())?;

        Ok(())
    }
}

/// Content page to display, swapping the empty state for the syncing
/// feedback while a history sync is in progress.
fn syncing_content_page(history_sync_active: bool, page: AppSessionPage) -> AppSessionPage {
    if history_sync_active && page == AppSessionPage::Empty {
        AppSessionPage::Syncing
    } else {
        page
    }
}

fn bp_with_setters(
    bp: adw::Breakpoint,
    additions: &[(&impl IsA<glib::Object>, &str, impl ToValue)],
) -> adw::Breakpoint {
    bp.add_setters(additions);
    bp
}
