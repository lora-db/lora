use serde::{Deserialize, Serialize};

use crate::error::{Result, SnapshotCodecError};
use crate::options::{Compression, PasswordKdfParams};
use crate::{SnapshotInfo, FORMAT_VERSION, HEADER_LEN, MAGIC};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format_version: u32,
    pub(crate) wal_lsn: Option<u64>,
    pub(crate) node_count: u64,
    pub(crate) relationship_count: u64,
    pub(crate) compression: Compression,
    pub(crate) encryption: EncryptionManifest,
    pub(crate) body_len: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum EncryptionManifest {
    None,
    ChaCha20Poly1305 {
        key_id: String,
        nonce: [u8; 12],
    },
    PasswordChaCha20Poly1305 {
        key_id: String,
        nonce: [u8; 12],
        salt: [u8; 16],
        params: PasswordKdfParams,
    },
}

pub(crate) fn encode_envelope(
    body: Vec<u8>,
    node_count: usize,
    relationship_count: usize,
    wal_lsn: Option<u64>,
    compression: Compression,
    encryption: EncryptionManifest,
) -> Result<(Vec<u8>, SnapshotInfo)> {
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        wal_lsn,
        node_count: node_count as u64,
        relationship_count: relationship_count as u64,
        compression,
        encryption,
        body_len: body.len() as u64,
    };
    let info = manifest_info(&manifest)?;
    let manifest_bytes =
        bincode::serialize(&manifest).map_err(|e| SnapshotCodecError::Encode(e.to_string()))?;
    if manifest_bytes.len() > u32::MAX as usize {
        return Err(SnapshotCodecError::Encode("manifest too large".into()));
    }

    let mut checksum_hasher = blake3::Hasher::new();
    checksum_hasher.update(&manifest_bytes);
    checksum_hasher.update(&body);
    let checksum = *checksum_hasher.finalize().as_bytes();

    let mut out = Vec::with_capacity(HEADER_LEN + manifest_bytes.len() + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(manifest_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&manifest_bytes);
    out.extend_from_slice(&body);
    Ok((out, info))
}

pub(crate) fn decode_envelope_borrowed(bytes: &[u8]) -> Result<(Manifest, &[u8])> {
    if bytes.len() < HEADER_LEN {
        return Err(SnapshotCodecError::Decode(
            "truncated snapshot header".into(),
        ));
    }
    if &bytes[0..8] != MAGIC {
        return Err(SnapshotCodecError::BadMagic);
    }
    let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(format_version));
    }
    let manifest_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let body_len = u64::from_le_bytes(bytes[16..24].try_into().unwrap()) as usize;
    let checksum: [u8; 32] = bytes[24..56].try_into().unwrap();
    let expected_len = HEADER_LEN
        .checked_add(manifest_len)
        .and_then(|len| len.checked_add(body_len))
        .ok_or_else(|| SnapshotCodecError::Decode("snapshot length overflow".into()))?;
    if bytes.len() != expected_len {
        return Err(SnapshotCodecError::Decode(format!(
            "snapshot length mismatch: expected {expected_len}, got {}",
            bytes.len()
        )));
    }

    let manifest_bytes = &bytes[HEADER_LEN..HEADER_LEN + manifest_len];
    let body = &bytes[HEADER_LEN + manifest_len..];
    let mut checksum_hasher = blake3::Hasher::new();
    checksum_hasher.update(manifest_bytes);
    checksum_hasher.update(body);
    if checksum_hasher.finalize().as_bytes() != &checksum {
        return Err(SnapshotCodecError::ChecksumMismatch);
    }

    let manifest: Manifest = bincode::deserialize(manifest_bytes)
        .map_err(|e| SnapshotCodecError::Decode(e.to_string()))?;
    Ok((manifest, body))
}

pub(crate) fn manifest_info(manifest: &Manifest) -> Result<SnapshotInfo> {
    Ok(SnapshotInfo {
        format_version: manifest.format_version,
        wal_lsn: manifest.wal_lsn,
        node_count: usize::try_from(manifest.node_count)
            .map_err(|_| SnapshotCodecError::Decode("node count overflows usize".into()))?,
        relationship_count: usize::try_from(manifest.relationship_count)
            .map_err(|_| SnapshotCodecError::Decode("relationship count overflows usize".into()))?,
        compression: manifest.compression,
        encrypted: !matches!(manifest.encryption, EncryptionManifest::None),
        key_id: match &manifest.encryption {
            EncryptionManifest::None => None,
            EncryptionManifest::ChaCha20Poly1305 { key_id, .. } => Some(key_id.clone()),
            EncryptionManifest::PasswordChaCha20Poly1305 { key_id, .. } => Some(key_id.clone()),
        },
    })
}
