use toasty::Db;

use crate::{
    DATA_DIR,
    db::{
        DbError,
        connection::open_encrypted_db,
        entities::{Chat, Contact, Message},
        keyring::KeyringService,
    },
};

/// Opens the per-session encrypted database file `{uuid}.session`.
///
/// The encryption key is fetched or created via the keyring service,
/// keyed by the session UUID. The database contains chat history,
/// messages, and contacts for a single `WhatsApp` account.
pub async fn open_session_db(uuid: &str, keyring: &KeyringService) -> Result<Db, DbError> {
    let key = keyring.get_or_create_session_key(uuid).await?;
    let path = DATA_DIR.join(format!("{uuid}.session"));

    open_encrypted_db(&path, &key, || toasty::models!(Chat, Message, Contact)).await
}
