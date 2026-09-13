pub mod app_version;
pub mod base_key;
pub mod device_list;
pub mod device_state;
pub mod group_metadata;
pub mod identity;
pub mod lid_mapping;
pub mod msg_secret;
pub mod mutation_mac;
pub mod pending_inbound;
pub mod pre_key;
pub mod sender_key;
pub mod sender_key_device;
pub mod sent_message;
pub mod signal_session;
pub mod signed_pre_key;
pub mod sync_key;
pub mod tc_token;

use toasty::schema::ModelSet;

pub use app_version::AppVersion;
pub use base_key::BaseKey;
pub use device_list::DeviceList;
pub use device_state::DeviceState;
pub use group_metadata::GroupMetadata;
pub use identity::Identity;
pub use lid_mapping::LidMapping;
pub use msg_secret::MsgSecret;
pub use mutation_mac::MutationMac;
pub use pending_inbound::PendingInbound;
pub use pre_key::PreKey;
pub use sender_key::SenderKey;
pub use sender_key_device::SenderKeyDevice;
pub use sent_message::SentMessage;
pub use signal_session::SignalSession;
pub use signed_pre_key::SignedPreKey;
pub use sync_key::SyncKey;
pub use tc_token::TcToken;

use super::entities::{Chat, Contact, Message};

pub(crate) fn session_models() -> ModelSet {
    toasty::models!(
        Chat,
        Message,
        Contact,
        AppVersion,
        BaseKey,
        DeviceList,
        DeviceState,
        GroupMetadata,
        Identity,
        LidMapping,
        MutationMac,
        MsgSecret,
        PendingInbound,
        PreKey,
        SenderKey,
        SenderKeyDevice,
        SentMessage,
        SignalSession,
        SignedPreKey,
        SyncKey,
        TcToken,
    )
}
