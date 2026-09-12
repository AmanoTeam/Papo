use std::fmt::{self, Write as _};

use oo7::{AsAttributes, Keyring};
use rand::Rng;

use crate::{i18n, i18n_f};

#[derive(Debug)]
pub enum KeyringError {
    Backend(Box<oo7::Error>),
}

impl fmt::Display for KeyringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for KeyringError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(&**error),
        }
    }
}

impl From<oo7::Error> for KeyringError {
    fn from(error: oo7::Error) -> Self {
        Self::Backend(Box::new(error))
    }
}

pub struct KeyringService {
    keyring: Keyring,
}

impl KeyringService {
    pub async fn new() -> Result<Self, KeyringError> {
        Ok(Self {
            keyring: Keyring::new().await?,
        })
    }

    fn with_keyring(keyring: Keyring) -> Self {
        Self { keyring }
    }

    pub async fn get_or_create_main_key(&self) -> Result<String, KeyringError> {
        self.get_or_create(&[("papo", "main")], &i18n!("Papo database encryption key"))
            .await
    }

    pub async fn get_or_create_session_key(&self, uuid: &str) -> Result<String, KeyringError> {
        self.get_or_create(
            &[("papo", "session"), ("uuid", uuid)],
            &i18n_f!("Papo session encryption key ({0})", &uuid),
        )
        .await
    }

    pub async fn delete_session_key(&self, uuid: &str) -> Result<(), KeyringError> {
        Ok(self
            .keyring
            .delete(&[("papo", "session"), ("uuid", uuid)])
            .await?)
    }

    async fn get_or_create(
        &self,
        attributes: &impl AsAttributes,
        label: &str,
    ) -> Result<String, KeyringError> {
        if let Some(item) = self.keyring.search_items(attributes).await?.pop() {
            let secret = item.secret().await?;
            if let Some(key) = secret.as_str() {
                return Ok(key.to_owned());
            }
        }

        let key = generate_key();
        self.keyring
            .create_item(label, attributes, key.as_str(), true)
            .await?;

        Ok(key)
    }
}

fn generate_key() -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);

    let mut key = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(key, "{byte:02x}");
    }

    key
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use oo7::{Keyring, Secret};
    use tokio::runtime::Runtime;
    use uuid::Uuid;

    use super::KeyringService;

    #[test]
    fn keyring_lifecycle() {
        Runtime::new().unwrap().block_on(async {
            let dir = env::temp_dir().join(format!("papo-keyring-test-{}", Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();

            let keyring =
                Keyring::sandboxed_with_path(dir.join("keyring"), Secret::random().unwrap())
                    .await
                    .unwrap();
            let service = KeyringService::with_keyring(keyring);

            let main = service.get_or_create_main_key().await.unwrap();
            assert_eq!(main.len(), 64);
            assert!(main.chars().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(service.get_or_create_main_key().await.unwrap(), main);

            let first = Uuid::new_v4().to_string();
            let second = Uuid::new_v4().to_string();
            let key_first = service.get_or_create_session_key(&first).await.unwrap();
            let key_second = service.get_or_create_session_key(&second).await.unwrap();
            assert_ne!(key_first, key_second);
            assert_eq!(
                service.get_or_create_session_key(&first).await.unwrap(),
                key_first
            );

            service.delete_session_key(&first).await.unwrap();
            let key_new = service.get_or_create_session_key(&first).await.unwrap();
            assert_ne!(key_new, key_first);

            fs::remove_dir_all(&dir).ok();
        });
    }
}
