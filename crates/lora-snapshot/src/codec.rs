//! Top-level encode/decode/read/write API for the columnar snapshot.
//!
//! The submodule files own the columnar layout (`columnar`), the
//! envelope framing (`envelope`), the body marshalling (`body`), the
//! compression and encryption transforms (`transform`), the
//! configuration vocabulary (`options`), and the zero-copy view
//! (`view`). This file is the public glue that strings them together.

use std::io::{Read, Write};

use lora_store::SnapshotPayload;

use crate::columnar::ColumnarSnapshot;
use crate::envelope::{
    decode_envelope_borrowed, encode_envelope, manifest_info, EncryptionManifest,
};
use crate::errors::{Result, SnapshotCodecError};
use crate::format::FORMAT_VERSION;
use crate::options::{Compression, SnapshotCredentials, SnapshotEncryption, SnapshotOptions};
use crate::transform::{compress, decompress, decrypt_body, derive_password_key, encrypt_body};
use crate::view::SnapshotView;

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
    let (body, encryption) = encode_snapshot_body(payload, wal_lsn, options)?;

    encode_envelope(
        body,
        payload.nodes.len(),
        payload.relationships.len(),
        wal_lsn,
        options.compression,
        encryption,
    )
}

fn encode_snapshot_body(
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<(Vec<u8>, EncryptionManifest)> {
    let columns = ColumnarSnapshot::from_payload(payload, wal_lsn)?;
    let body = columns.encode_binary()?;
    let body = compress(body, options.compression)?;
    encrypt_snapshot_body(body, options.encryption.as_ref())
}

pub fn snapshot_info(bytes: &[u8]) -> Result<SnapshotInfo> {
    let (manifest, _) = decode_envelope_borrowed(bytes)?;
    manifest_info(&manifest)
}

pub fn open_snapshot_view(bytes: &[u8]) -> Result<SnapshotView<'_>> {
    let (manifest, body) = decode_envelope_borrowed(bytes)?;
    ensure_supported_format(manifest.format_version)?;
    if !supports_zero_copy_view(manifest.compression, &manifest.encryption) {
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
    ensure_supported_format(manifest.format_version)?;

    let body = decode_snapshot_body(
        body,
        manifest.compression,
        &manifest.encryption,
        credentials,
    )?;
    Ok((
        snapshot_payload_from_body(&body)?,
        manifest_info(&manifest)?,
    ))
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

fn ensure_supported_format(format_version: u32) -> Result<()> {
    if format_version == FORMAT_VERSION {
        Ok(())
    } else {
        Err(SnapshotCodecError::UnsupportedVersion(format_version))
    }
}

fn supports_zero_copy_view(compression: Compression, encryption: &EncryptionManifest) -> bool {
    compression == Compression::None && matches!(encryption, EncryptionManifest::None)
}

fn decode_snapshot_body(
    body: &[u8],
    compression: Compression,
    encryption: &EncryptionManifest,
    credentials: Option<&SnapshotCredentials>,
) -> Result<Vec<u8>> {
    let body = decrypt_snapshot_body(body, encryption, credentials)?;
    decompress(body, compression)
}

fn snapshot_payload_from_body(body: &[u8]) -> Result<SnapshotPayload> {
    ColumnarSnapshot::decode_binary(body)?.into_payload()
}

fn encrypt_snapshot_body(
    mut body: Vec<u8>,
    encryption: Option<&SnapshotEncryption>,
) -> Result<(Vec<u8>, EncryptionManifest)> {
    let Some(encryption) = encryption else {
        return Ok((body, EncryptionManifest::None));
    };

    let nonce = random_bytes("nonce")?;

    let manifest = match encryption {
        SnapshotEncryption::Key(key) => {
            body = encrypt_body(&body, &key.key, &nonce)?;
            EncryptionManifest::ChaCha20Poly1305 {
                key_id: key.key_id.clone(),
                nonce,
            }
        }
        SnapshotEncryption::Password(password) => {
            let salt = random_bytes("salt")?;
            let key = derive_password_key(password.password.as_slice(), &salt, password.params)?;
            body = encrypt_body(&body, &key, &nonce)?;
            EncryptionManifest::PasswordChaCha20Poly1305 {
                key_id: password.key_id.clone(),
                nonce,
                salt,
                params: password.params,
            }
        }
    };

    Ok((body, manifest))
}

fn random_bytes<const N: usize>(label: &str) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| SnapshotCodecError::Encode(format!("{label} generation failed: {e}")))?;
    Ok(bytes)
}

fn decrypt_snapshot_body(
    body: &[u8],
    encryption: &EncryptionManifest,
    credentials: Option<&SnapshotCredentials>,
) -> Result<Vec<u8>> {
    match encryption {
        EncryptionManifest::None => Ok(body.to_vec()),
        EncryptionManifest::ChaCha20Poly1305 { key_id, nonce } => {
            let key = matching_key_credentials(credentials, key_id)?;
            decrypt_body(body, &key.key, nonce)
        }
        EncryptionManifest::PasswordChaCha20Poly1305 {
            key_id,
            nonce,
            salt,
            params,
        } => {
            let password = matching_password_credentials(credentials, key_id)?;
            let key = derive_password_key(password.password.as_slice(), salt, *params)?;
            decrypt_body(body, &key, nonce)
        }
    }
}

fn matching_key_credentials<'a>(
    credentials: Option<&'a SnapshotCredentials>,
    key_id: &str,
) -> Result<&'a crate::options::EncryptionKey> {
    match credentials {
        Some(SnapshotEncryption::Key(key)) if key.key_id == key_id => Ok(key),
        _ => Err(SnapshotCodecError::MissingEncryptionKey(key_id.to_string())),
    }
}

fn matching_password_credentials<'a>(
    credentials: Option<&'a SnapshotCredentials>,
    key_id: &str,
) -> Result<&'a crate::options::SnapshotPassword> {
    match credentials {
        Some(SnapshotEncryption::Password(password)) if password.key_id == key_id => Ok(password),
        _ => Err(SnapshotCodecError::MissingPassword(key_id.to_string())),
    }
}
