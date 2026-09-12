pub mod connection;
pub mod entities;
pub mod keyring;
pub mod migrations;
pub mod session;

use std::{fmt, io};

use self::keyring::KeyringError;

#[derive(Debug)]
pub enum DbError {
    Io(io::Error),
    Toasty(Box<toasty::Error>),
    Keyring(KeyringError),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Toasty(error) => write!(f, "{error}"),
            Self::Keyring(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Toasty(error) => Some(error.as_ref()),
            Self::Keyring(error) => Some(error),
        }
    }
}

impl From<io::Error> for DbError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<toasty::Error> for DbError {
    fn from(error: toasty::Error) -> Self {
        Self::Toasty(Box::new(error))
    }
}

impl From<KeyringError> for DbError {
    fn from(error: KeyringError) -> Self {
        Self::Keyring(error)
    }
}
