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

pub async fn open_session_db(uuid: &str, keyring: &KeyringService) -> Result<Db, DbError> {
    let key = keyring.get_or_create_session_key(uuid).await?;
    let path = DATA_DIR.join(format!("{uuid}.session"));

    open_encrypted_db(&path, &key, || toasty::models!(Chat, Message, Contact)).await
}
