use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use toasty::{Db, schema::ModelSet};
use toasty_driver_turso::{EncryptionOpts, Turso};

use crate::{
    DATA_DIR,
    db::{DbError, entities::Session, keyring::KeyringService},
};

pub async fn open_main_db(keyring: &KeyringService) -> Result<Db, DbError> {
    let key = keyring.get_or_create_main_key().await?;
    let path = DATA_DIR.join("papo.db");

    open_encrypted_db(&path, &key, || toasty::models!(Session)).await
}

pub(crate) async fn open_encrypted_db(
    path: &Path,
    hexkey: &str,
    models: impl Fn() -> ModelSet,
) -> Result<Db, DbError> {
    let driver = create_driver(path, hexkey);

    if let Ok(db) = Db::builder().models(models()).build(driver).await {
        db.push_schema().await?;
        return Ok(db);
    }

    quarantine_file(path);
    let driver = create_driver(path, hexkey);
    let db = Db::builder().models(models()).build(driver).await?;
    db.push_schema().await?;

    Ok(db)
}

pub(crate) fn create_driver(path: &Path, hexkey: &str) -> Turso {
    Turso::file(path).experimental_encryption(EncryptionOpts {
        cipher: "aes256gcm".into(),
        hexkey: hexkey.into(),
    })
}

pub(crate) fn quarantine_file(path: &Path) {
    if !path.exists() {
        return;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    let corrupt = path.with_extension(format!("corrupt-{timestamp}"));
    let _ = fs::rename(path, &corrupt);
    let _ = fs::remove_file(format!("{}-wal", path.display()));
    let _ = fs::remove_file(format!("{}-shm", path.display()));
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use tokio::runtime::Runtime;
    use uuid::Uuid;

    use super::*;
    use crate::db::entities::{Chat, Message};

    #[test]
    fn fresh_create_and_crud() {
        Runtime::new().unwrap().block_on(async {
            let db = Db::builder()
                .models(toasty::models!(Chat, Message))
                .build(Turso::in_memory())
                .await
                .unwrap();
            db.push_schema().await.unwrap();
        });
    }

    #[test]
    fn encrypted_at_rest() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let dir = env::temp_dir().join(format!("papo-encrypt-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();

            let path = dir.join("test.db");
            let key = "0".repeat(64);

            let driver = create_driver(&path, &key);
            let db = Db::builder()
                .models(toasty::models!(Chat, Message))
                .build(driver)
                .await
                .unwrap();
            db.push_schema().await.unwrap();

            let header = fs::read(&path).unwrap();
            assert!(!header.starts_with(b"SQLite format 3"));

            fs::remove_dir_all(&dir).ok();
        });
    }

    #[test]
    fn quarantine_renames_file() {
        let dir = env::temp_dir().join(format!("papo-quarantine-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("test.db");
        fs::write(&path, b"garbage").unwrap();

        assert!(path.exists());
        quarantine_file(&path);
        assert!(!path.exists());

        let quarantined = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("test.corrupt-")
            })
            .count();
        assert_eq!(quarantined, 1);

        fs::remove_dir_all(&dir).ok();
    }
}
