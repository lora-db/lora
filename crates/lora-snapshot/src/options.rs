use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotOptions {
    pub compression: Compression,
    pub encryption: Option<SnapshotEncryption>,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            compression: Compression::Gzip { level: 1 },
            encryption: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compression {
    None,
    Gzip { level: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionKey {
    pub key_id: String,
    pub key: [u8; 32],
}

impl EncryptionKey {
    pub fn new(key_id: impl Into<String>, key: [u8; 32]) -> Self {
        Self {
            key_id: key_id.into(),
            key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPassword {
    pub key_id: String,
    pub(crate) password: Vec<u8>,
    pub params: PasswordKdfParams,
}

impl SnapshotPassword {
    pub fn new(key_id: impl Into<String>, password: impl AsRef<[u8]>) -> Self {
        Self {
            key_id: key_id.into(),
            password: password.as_ref().to_vec(),
            params: PasswordKdfParams::interactive(),
        }
    }

    pub fn with_params(
        key_id: impl Into<String>,
        password: impl AsRef<[u8]>,
        params: PasswordKdfParams,
    ) -> Self {
        Self {
            key_id: key_id.into(),
            password: password.as_ref().to_vec(),
            params,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordKdfParams {
    pub memory_cost_kib: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl PasswordKdfParams {
    pub fn interactive() -> Self {
        Self {
            memory_cost_kib: 19 * 1024,
            time_cost: 2,
            parallelism: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotEncryption {
    Key(EncryptionKey),
    Password(SnapshotPassword),
}

impl From<EncryptionKey> for SnapshotEncryption {
    fn from(value: EncryptionKey) -> Self {
        Self::Key(value)
    }
}

impl From<SnapshotPassword> for SnapshotEncryption {
    fn from(value: SnapshotPassword) -> Self {
        Self::Password(value)
    }
}

pub type SnapshotCredentials = SnapshotEncryption;
