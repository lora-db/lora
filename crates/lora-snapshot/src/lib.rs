//! Efficient snapshots for LoraDB graph state.
//!
//! This crate is intentionally separate from `lora-store` and `lora-wal`:
//! the store owns the canonical in-memory records, the WAL owns ordered
//! mutation recovery, and this crate owns compact point-in-time state images.
//!
//! The current format is column-oriented rather than bincode-over-struct:
//! nodes, labels, relationships, relationship types, and properties are stored
//! in separate columns. That keeps the format friendly to future Arrow /
//! Parquet backends while avoiding those heavy dependencies in the first
//! implementation. Compression and authenticated encryption are applied to the
//! encoded column body.

use std::io::{Read, Write};

use lora_store::SnapshotPayload;

mod body;
mod columnar;
mod envelope;
mod error;
mod options;
#[cfg(test)]
mod tests;
mod transform;
mod view;

use columnar::ColumnarSnapshot;
use envelope::{decode_envelope_borrowed, encode_envelope, manifest_info, EncryptionManifest};
use transform::{compress, decompress, decrypt_body, derive_password_key, encrypt_body};

pub use error::{Result, SnapshotCodecError};
pub use options::{
    Compression, EncryptionKey, PasswordKdfParams, SnapshotCredentials, SnapshotEncryption,
    SnapshotOptions, SnapshotPassword,
};
pub use view::{SnapshotView, StringTableView, U32ColumnView, U64ColumnView};

pub(crate) const MAGIC: &[u8; 8] = b"LORACOL1";
pub const DATABASE_SNAPSHOT_MAGIC: &[u8; 8] = MAGIC;
pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 32;
pub(crate) const BODY_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub format_version: u32,
    pub wal_lsn: Option<u64>,
    pub node_count: usize,
    pub relationship_count: usize,
    pub compression: Compression,
    pub encrypted: bool,
    pub key_id: Option<String>,
}

pub fn encode_snapshot(payload: &SnapshotPayload, wal_lsn: Option<u64>) -> Result<Vec<u8>> {
    encode_snapshot_with_options(payload, wal_lsn, &SnapshotOptions::default())
}

pub fn encode_snapshot_with_options(
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<Vec<u8>> {
    encode_snapshot_with_options_and_info(payload, wal_lsn, options).map(|(bytes, _)| bytes)
}

fn encode_snapshot_with_options_and_info(
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<(Vec<u8>, SnapshotInfo)> {
    let columns = ColumnarSnapshot::from_payload(payload, wal_lsn);
    let mut body = columns.encode_binary()?;
    body = compress(body, options.compression)?;

    let encryption = if let Some(encryption) = &options.encryption {
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce)
            .map_err(|e| SnapshotCodecError::Encode(format!("nonce generation failed: {e}")))?;
        match encryption {
            SnapshotEncryption::Key(key) => {
                body = encrypt_body(&body, &key.key, &nonce)?;
                EncryptionManifest::ChaCha20Poly1305 {
                    key_id: key.key_id.clone(),
                    nonce,
                }
            }
            SnapshotEncryption::Password(password) => {
                let mut salt = [0u8; 16];
                getrandom::getrandom(&mut salt).map_err(|e| {
                    SnapshotCodecError::Encode(format!("salt generation failed: {e}"))
                })?;
                let key =
                    derive_password_key(password.password.as_slice(), &salt, password.params)?;
                body = encrypt_body(&body, &key, &nonce)?;
                EncryptionManifest::PasswordChaCha20Poly1305 {
                    key_id: password.key_id.clone(),
                    nonce,
                    salt,
                    params: password.params,
                }
            }
        }
    } else {
        EncryptionManifest::None
    };

    encode_envelope(
        body,
        payload.nodes.len(),
        payload.relationships.len(),
        wal_lsn,
        options.compression,
        encryption,
    )
}

pub fn snapshot_info(bytes: &[u8]) -> Result<SnapshotInfo> {
    let (manifest, _) = decode_envelope_borrowed(bytes)?;
    manifest_info(&manifest)
}

pub fn open_snapshot_view(bytes: &[u8]) -> Result<SnapshotView<'_>> {
    let (manifest, body) = decode_envelope_borrowed(bytes)?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(
            manifest.format_version,
        ));
    }
    if manifest.compression != Compression::None
        || !matches!(manifest.encryption, EncryptionManifest::None)
    {
        return Err(SnapshotCodecError::Decode(
            "zero-copy views require uncompressed, unencrypted snapshots".into(),
        ));
    }
    SnapshotView::parse(manifest_info(&manifest)?, body)
}

pub fn decode_snapshot(
    bytes: &[u8],
    credentials: Option<&SnapshotCredentials>,
) -> Result<(SnapshotPayload, SnapshotInfo)> {
    let (manifest, body) = decode_envelope_borrowed(bytes)?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(
            manifest.format_version,
        ));
    }

    let mut body = match &manifest.encryption {
        EncryptionManifest::None => body.to_vec(),
        EncryptionManifest::ChaCha20Poly1305 { key_id, nonce } => {
            let key = match credentials {
                Some(SnapshotEncryption::Key(key)) if key.key_id == *key_id => key,
                _ => return Err(SnapshotCodecError::MissingEncryptionKey(key_id.clone())),
            };
            decrypt_body(body, &key.key, nonce)?
        }
        EncryptionManifest::PasswordChaCha20Poly1305 {
            key_id,
            nonce,
            salt,
            params,
        } => {
            let password = match credentials {
                Some(SnapshotEncryption::Password(password)) if password.key_id == *key_id => {
                    password
                }
                _ => return Err(SnapshotCodecError::MissingPassword(key_id.clone())),
            };
            let key = derive_password_key(password.password.as_slice(), salt, *params)?;
            decrypt_body(body, &key, nonce)?
        }
    };
    body = decompress(body, manifest.compression)?;

    let columns = ColumnarSnapshot::decode_binary(&body)?;
    let payload = columns.into_payload()?;
    let info = manifest_info(&manifest)?;
    Ok((payload, info))
}

pub fn write_snapshot<W: Write>(
    mut writer: W,
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<SnapshotInfo> {
    let (bytes, info) = encode_snapshot_with_options_and_info(payload, wal_lsn, options)?;
    writer.write_all(&bytes)?;
    Ok(info)
}

pub fn read_snapshot<R: Read>(
    mut reader: R,
    credentials: Option<&SnapshotCredentials>,
) -> Result<(SnapshotPayload, SnapshotInfo)> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    decode_snapshot(&bytes, credentials)
}
