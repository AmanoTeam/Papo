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
use toasty_driver_sqlite::Sqlite;

use crate::{
    DATA_DIR,
    db::{DbError, entities::Session, keyring::KeyringService},
};

/// Opens the central `papo.db` database with `SQLCipher` encryption.
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
/// opened as-is. If the file is corrupt, was encrypted with a different
/// key, or was written by a different database engine, it is quarantined
/// (renamed to `{name}.corrupt-{timestamp}`) and a fresh database is
/// created in its place.
pub(crate) async fn open_encrypted_db(
    path: &Path,
    hexkey: &str,
    models: impl Fn() -> ModelSet,
) -> Result<Db, DbError> {
    let is_fresh = !path.exists();
    let driver = Sqlite::open_encrypted(path, hexkey);

    if let Ok(mut db) = Db::builder()
        .max_pool_size(1)
        .models(models())
        .build(driver)
        .await
    {
        if is_fresh {
            db.push_schema().await?;
        } else if db_is_readable(&mut db).await {
            ensure_indices(&mut db).await;
        } else {
            quarantine_file(path);
            return recreate_encrypted_db(path, hexkey, models).await;
        }

        set_sync_mode(&mut db).await;

        return Ok(db);
    }

    quarantine_file(path);

    recreate_encrypted_db(path, hexkey, models).await
}

/// Creates a fresh encrypted database on a freshly quarantined path.
async fn recreate_encrypted_db(
    path: &Path,
    hexkey: &str,
    models: impl Fn() -> ModelSet,
) -> Result<Db, DbError> {
    let driver = Sqlite::open_encrypted(path, hexkey);
    let mut db = Db::builder()
        .max_pool_size(1)
        .models(models())
        .build(driver)
        .await?;
    db.push_schema().await?;
    set_sync_mode(&mut db).await;

    Ok(db)
}

/// Proves the engine can actually read the file. Touching
/// `sqlite_master` decrypts the first page, so an unreadable database
/// fails here instead of at an arbitrary later query. `LIMIT 0` keeps
/// the statement row-free.
async fn db_is_readable(db: &mut Db) -> bool {
    db.exec_raw_sql(RawSql {
        sql: "SELECT * FROM sqlite_master LIMIT 0".to_owned(),
        ret: RawSqlRet::None,
        params: Vec::new(),
    })
    .await
    .is_ok()
}

/// Relaxes the WAL fsync mode. With WAL, committed transactions only need
/// to survive an app crash, and `synchronous = NORMAL` lets the periodic
/// checkpoint carry the fsync instead. The pragma is per-connection, but
/// the pool is capped at a single connection for the whole session, so one
/// call covers every statement.
async fn set_sync_mode(db: &mut Db) {
    let result = db
        .exec_raw_sql(RawSql {
            sql: "PRAGMA synchronous = NORMAL".to_owned(),
            ret: RawSqlRet::None,
            params: Vec::new(),
        })
        .await;

    if let Err(e) = result {
        tracing::warn!("Failed to relax the database sync mode: {e}");
    }
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

    use toasty_core::{driver::operation::TypedValue, schema::db::Type, stmt::Value as SqlValue};
    use tokio::runtime::Runtime;
    use uuid::Uuid;
    use whatsapp_rust::wacore::{
        appstate::processor::AppStateMutationMAC, store::traits::LidPnMappingEntry,
    };

    use super::*;
    use crate::db::{
        entities::{Chat, Message},
        protocol::{LidMapping, MutationMac, TcToken},
    };

    #[test]
    fn fresh_create_and_crud() {
        Runtime::new().unwrap().block_on(async {
            let db = Db::builder()
                .models(toasty::models!(Chat, Message))
                .build(Sqlite::in_memory())
                .await
                .unwrap();
            db.push_schema().await.unwrap();
        });
    }

    /// Checks the batched multi-row lid-mapping upsert used by
    /// `put_lid_mappings` against a real engine: table and column names,
    /// positional placeholders, and the conflict clause.
    #[test]
    fn batched_lid_mapping_upsert() {
        Runtime::new().unwrap().block_on(async {
            let mut db = Db::builder()
                .models(toasty::models!(LidMapping))
                .build(Sqlite::in_memory())
                .await
                .unwrap();
            db.push_schema().await.unwrap();

            let row = |lid: &str, phone: &str| LidPnMappingEntry {
                lid: lid.to_string(),
                phone_number: phone.to_string(),
                created_at: 1,
                updated_at: 2,
                learning_source: "history".to_string(),
            };

            let entries = vec![row("lid-1", "111"), row("lid-2", "222")];

            let mut sql = String::from(
                "INSERT INTO \"lid_mappings\" \
                 (\"lid\", \"created_at\", \"updated_at\", \"phone_number\", \"learning_source\") VALUES ",
            );
            let mut params = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push_str("(?, ?, ?, ?, ?)");
                params.extend([
                    TypedValue {
                        value: SqlValue::String(entry.lid.clone()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::I64(entry.created_at),
                        ty: Type::Integer(8),
                    },
                    TypedValue {
                        value: SqlValue::I64(entry.updated_at),
                        ty: Type::Integer(8),
                    },
                    TypedValue {
                        value: SqlValue::String(entry.phone_number.clone()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::String(entry.learning_source.clone()),
                        ty: Type::Text,
                    },
                ]);
            }
            sql.push_str(
                " ON CONFLICT(\"lid\") DO UPDATE SET \
                 \"created_at\" = excluded.\"created_at\", \
                 \"updated_at\" = excluded.\"updated_at\", \
                 \"phone_number\" = excluded.\"phone_number\", \
                 \"learning_source\" = excluded.\"learning_source\"",
            );

            db.exec_raw_sql(RawSql {
                sql,
                params,
                ret: RawSqlRet::None,
            })
            .await
            .unwrap();

            let count = LidMapping::all().exec(&mut db).await.unwrap().len();
            assert_eq!(count, 2);

            // Upserting the same lids must overwrite, not duplicate.
            let mut updated = entries;
            updated[1].phone_number = "999".to_string();
            let conflict_row = &updated[1];

            db.exec_raw_sql(RawSql {
                sql: "INSERT INTO \"lid_mappings\" (\"lid\", \"created_at\", \"updated_at\", \"phone_number\", \"learning_source\") \
                      VALUES (?, ?, ?, ?, ?) \
                      ON CONFLICT(\"lid\") DO UPDATE SET \
                      \"created_at\" = excluded.\"created_at\", \
                      \"updated_at\" = excluded.\"updated_at\", \
                      \"phone_number\" = excluded.\"phone_number\", \
                      \"learning_source\" = excluded.\"learning_source\""
                    .to_string(),
                params: vec![
                    TypedValue {
                        value: SqlValue::String(conflict_row.lid.clone()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::I64(conflict_row.created_at),
                        ty: Type::Integer(8),
                    },
                    TypedValue {
                        value: SqlValue::I64(conflict_row.updated_at),
                        ty: Type::Integer(8),
                    },
                    TypedValue {
                        value: SqlValue::String(conflict_row.phone_number.clone()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::String(conflict_row.learning_source.clone()),
                        ty: Type::Text,
                    },
                ],
                ret: RawSqlRet::None,
            })
            .await
            .unwrap();

            let all = LidMapping::all().exec(&mut db).await.unwrap();
            assert_eq!(all.len(), 2);
            assert!(all
                .iter()
                .any(|m| m.lid == "lid-2" && m.phone_number == "999"));
        });
    }

    /// Runs the exact production multi-row `mutation_macs` upsert against the
    /// real engine: table and column names, the composite conflict target and
    /// the blob binding.
    #[test]
    fn batched_mutation_mac_upsert() {
        Runtime::new().unwrap().block_on(async {
            let mut db = Db::builder()
                .models(toasty::models!(MutationMac))
                .build(Sqlite::in_memory())
                .await
                .unwrap();
            db.push_schema().await.unwrap();

            let hex = |bytes: &[u8]| -> String {
                bytes.iter().map(|byte| format!("{byte:02x}")).collect()
            };

            let row = |index_mac: &[u8], value_mac: &[u8]| AppStateMutationMAC {
                index_mac: index_mac.to_vec(),
                value_mac: value_mac.to_vec(),
            };
            let mutations = vec![row(&[1, 2, 3], &[9]), row(&[4, 5, 6], &[8])];

            let mut sql = String::from(
                "INSERT INTO \"mutation_macs\" (\"name\", \"index_mac\", \"value_mac\") VALUES ",
            );
            let mut params = Vec::new();
            for (index, mutation) in mutations.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push_str("(?, ?, ?)");
                params.extend([
                    TypedValue {
                        value: SqlValue::String("critical_block".to_string()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::String(hex(&mutation.index_mac)),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::Bytes(mutation.value_mac.clone()),
                        ty: Type::Blob,
                    },
                ]);
            }
            sql.push_str(
                " ON CONFLICT(\"name\", \"index_mac\") \
                 DO UPDATE SET \"value_mac\" = excluded.\"value_mac\"",
            );

            db.exec_raw_sql(RawSql {
                sql,
                params,
                ret: RawSqlRet::None,
            })
            .await
            .unwrap();

            let count = MutationMac::all().exec(&mut db).await.unwrap().len();
            assert_eq!(count, 2);

            // Upserting the same (name, index_mac) must overwrite the value.
            db.exec_raw_sql(RawSql {
                sql: "INSERT INTO \"mutation_macs\" (\"name\", \"index_mac\", \"value_mac\") \
                      VALUES (?, ?, ?) \
                      ON CONFLICT(\"name\", \"index_mac\") DO UPDATE SET \
                      \"value_mac\" = excluded.\"value_mac\""
                    .to_string(),
                params: vec![
                    TypedValue {
                        value: SqlValue::String("critical_block".to_string()),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::String(hex(&[1, 2, 3])),
                        ty: Type::Text,
                    },
                    TypedValue {
                        value: SqlValue::Bytes(vec![7]),
                        ty: Type::Blob,
                    },
                ],
                ret: RawSqlRet::None,
            })
            .await
            .unwrap();

            let all = MutationMac::all().exec(&mut db).await.unwrap();
            assert_eq!(all.len(), 2);
            assert!(
                all.iter()
                    .any(|m| m.index_mac == hex(&[1, 2, 3]) && m.value_mac == vec![7])
            );
        });
    }

    #[test]
    fn encrypted_at_rest() {
        Runtime::new().unwrap().block_on(async {
            let dir = env::temp_dir().join(format!("papo-encrypt-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();

            let path = dir.join("test.db");
            let key = "0".repeat(64);

            let driver = Sqlite::open_encrypted(&path, &key);
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
