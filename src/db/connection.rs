use std::{
    ffi::OsStr,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use toasty::{
    Db, Executor,
    schema::{ModelSet, db::IndexOp},
};
use toasty_core::{
    driver::operation::{RawSql, RawSqlRet},
    stmt::Direction,
};
use toasty_driver_turso::{EncryptionOpts, Turso};

use crate::{
    DATA_DIR,
    db::{DbError, entities::Session, keyring::KeyringService},
};

/// Opens the central `papo.db` database with AES-256-GCM encryption.
///
/// The encryption key is fetched or created via the keyring service.
/// The database stores session metadata (one row per `WhatsApp` account).
pub async fn open_main_db(keyring: &KeyringService) -> Result<Db, DbError> {
    let key = keyring.get_or_create_main_key().await?;
    let path = DATA_DIR.join("papo.db");

    open_encrypted_db(&path, &key, || toasty::models!(Session)).await
}

/// Opens (or creates) an encrypted database file with the given models.
///
/// The schema is only pushed on first creation; existing databases are
/// opened as-is. If the file is corrupt or was encrypted with a different
/// key, it is quarantined (renamed to `{name}.corrupt-{timestamp}`) and
/// a fresh database is created in its place.
pub(crate) async fn open_encrypted_db(
    path: &Path,
    hexkey: &str,
    models: impl Fn() -> ModelSet,
) -> Result<Db, DbError> {
    let is_fresh = !path.exists();
    let driver = create_driver(path, hexkey);

    if let Ok(mut db) = Db::builder()
        .models(models())
        .max_pool_size(2)
        .build(driver)
        .await
    {
        if is_fresh {
            db.push_schema().await?;
        } else {
            ensure_indices(&mut db).await;
        }

        return Ok(db);
    }

    quarantine_file(path);
    let driver = create_driver(path, hexkey);
    let db = Db::builder()
        .models(models())
        .max_pool_size(2)
        .build(driver)
        .await?;
    db.push_schema().await?;

    Ok(db)
}

/// Creates any index declared in the schema but missing from an existing
/// database.
async fn ensure_indices(db: &mut Db) {
    let schema = db.schema().clone();
    for table in &schema.db.tables {
        for index in &table.indices {
            if index.primary_key {
                continue;
            }

            let unique = if index.unique { "UNIQUE " } else { "" };
            let columns = index
                .columns
                .iter()
                .map(|column| {
                    let name = &column.table_column(&schema.db).name;
                    if matches!(column.op, IndexOp::Sort(Direction::Desc)) {
                        format!("\"{name}\" DESC")
                    } else {
                        format!("\"{name}\"")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");

            let sql = format!(
                "CREATE {unique}INDEX IF NOT EXISTS \"{}\" ON \"{}\" ({columns})",
                index.name, table.name
            );

            let result = db
                .exec_raw_sql(RawSql {
                    sql,
                    ret: RawSqlRet::None,
                    params: Vec::new(),
                })
                .await;

            if let Err(e) = result {
                tracing::warn!("Failed to ensure index {}: {e}", index.name);
            }
        }
    }
}

/// Creates a Turso driver with AES-256-GCM page encryption enabled.
pub(crate) fn create_driver(path: &Path, hexkey: &str) -> Turso {
    Turso::file(path)
        .concurrent_writes()
        .experimental_encryption(EncryptionOpts {
            cipher: "aes256gcm".into(),
            hexkey: hexkey.into(),
        })
}

/// Renames a corrupt or unreadable database file to `{name}.corrupt-{timestamp}`
/// and removes its WAL/SHM sidecars. Does nothing if the file does not exist.
pub(crate) fn quarantine_file(path: &Path) {
    if !path.exists() {
        return;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    remove_sidecars(path);

    let corrupt = path.with_extension(format!("corrupt-{timestamp}"));
    let _ = fs::rename(path, &corrupt);
}

/// Removes journal sidecar files sharing the database file's stem,
/// leaving the main file itself intact.
fn remove_sidecars(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
        return;
    };

    let main_name = path.file_name().unwrap_or_default();
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if name != main_name && name.to_str().is_some_and(|name| name.starts_with(stem)) {
            let _ = fs::remove_file(entry.path());
        }
    }
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
        Runtime::new().unwrap().block_on(async {
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
    fn reopen_existing_database() {
        Runtime::new().unwrap().block_on(async {
            let dir = env::temp_dir().join(format!("papo-reopen-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();

            let path = dir.join("test.db");
            let key = "2".repeat(64);

            // First open: fresh file, schema pushed.
            let db = open_encrypted_db(&path, &key, || toasty::models!(Chat, Message))
                .await
                .unwrap();
            drop(db);

            // Second open: existing file, schema push must be skipped.
            open_encrypted_db(&path, &key, || toasty::models!(Chat, Message))
                .await
                .unwrap();

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
