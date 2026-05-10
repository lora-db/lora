use crate::body::{write_bytes, write_string, write_u32, write_u64, BodyReader};
use crate::codec::SnapshotInfo;
use crate::errors::{Result, SnapshotCodecError};
use crate::format::{FORMAT_VERSION, HEADER_LEN, MAGIC};
use crate::options::{Compression, PasswordKdfParams};

#[derive(Debug)]
pub(crate) struct Manifest {
    pub(crate) format_version: u32,
    pub(crate) wal_lsn: Option<u64>,
    pub(crate) node_count: u64,
    pub(crate) relationship_count: u64,
    pub(crate) compression: Compression,
    pub(crate) encryption: EncryptionManifest,
    pub(crate) body_len: u64,
}

#[derive(Debug)]
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
    let manifest_bytes = encode_manifest(&manifest)?;
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
    let format_version = u32::from_le_bytes(read_header_array::<4>(bytes, 8)?);
    if format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(format_version));
    }
    let manifest_len = u32::from_le_bytes(read_header_array::<4>(bytes, 12)?) as usize;
    let body_len = u64::from_le_bytes(read_header_array::<8>(bytes, 16)?) as usize;
    let checksum = read_header_array::<32>(bytes, 24)?;
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

    let manifest = decode_manifest(manifest_bytes)?;
    Ok((manifest, body))
}

const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_GZIP: u8 = 1;

const ENCRYPTION_NONE: u8 = 0;
const ENCRYPTION_CHACHA20_POLY1305: u8 = 1;
const ENCRYPTION_PASSWORD_CHACHA20_POLY1305: u8 = 2;

fn encode_manifest(manifest: &Manifest) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_u32(&mut out, manifest.format_version);
    match manifest.wal_lsn {
        Some(lsn) => {
            out.push(1);
            write_u64(&mut out, lsn);
        }
        None => out.push(0),
    }
    write_u64(&mut out, manifest.node_count);
    write_u64(&mut out, manifest.relationship_count);
    write_compression(&mut out, manifest.compression);
    write_encryption(&mut out, &manifest.encryption)?;
    write_u64(&mut out, manifest.body_len);
    Ok(out)
}

fn decode_manifest(bytes: &[u8]) -> Result<Manifest> {
    let mut reader = BodyReader::new(bytes);
    let format_version = reader.read_u32()?;
    let wal_lsn = match reader.read_u8()? {
        0 => None,
        1 => Some(reader.read_u64()?),
        tag => {
            return Err(SnapshotCodecError::Decode(format!(
                "invalid wal_lsn presence tag {tag}"
            )));
        }
    };
    let manifest = Manifest {
        format_version,
        wal_lsn,
        node_count: reader.read_u64()?,
        relationship_count: reader.read_u64()?,
        compression: read_compression(&mut reader)?,
        encryption: read_encryption(&mut reader)?,
        body_len: reader.read_u64()?,
    };
    reader.finish()?;
    Ok(manifest)
}

fn write_compression(out: &mut Vec<u8>, compression: Compression) {
    match compression {
        Compression::None => out.push(COMPRESSION_NONE),
        Compression::Gzip { level } => {
            out.push(COMPRESSION_GZIP);
            write_u32(out, level);
        }
    }
}

fn read_compression(reader: &mut BodyReader<'_>) -> Result<Compression> {
    match reader.read_u8()? {
        COMPRESSION_NONE => Ok(Compression::None),
        COMPRESSION_GZIP => Ok(Compression::Gzip {
            level: reader.read_u32()?,
        }),
        tag => Err(SnapshotCodecError::Decode(format!(
            "invalid compression tag {tag}"
        ))),
    }
}

fn write_encryption(out: &mut Vec<u8>, encryption: &EncryptionManifest) -> Result<()> {
    match encryption {
        EncryptionManifest::None => out.push(ENCRYPTION_NONE),
        EncryptionManifest::ChaCha20Poly1305 { key_id, nonce } => {
            out.push(ENCRYPTION_CHACHA20_POLY1305);
            write_string(out, key_id)?;
            write_bytes(out, nonce)?;
        }
        EncryptionManifest::PasswordChaCha20Poly1305 {
            key_id,
            nonce,
            salt,
            params,
        } => {
            out.push(ENCRYPTION_PASSWORD_CHACHA20_POLY1305);
            write_string(out, key_id)?;
            write_bytes(out, nonce)?;
            write_bytes(out, salt)?;
            write_password_params(out, *params);
        }
    }
    Ok(())
}

fn read_encryption(reader: &mut BodyReader<'_>) -> Result<EncryptionManifest> {
    match reader.read_u8()? {
        ENCRYPTION_NONE => Ok(EncryptionManifest::None),
        ENCRYPTION_CHACHA20_POLY1305 => Ok(EncryptionManifest::ChaCha20Poly1305 {
            key_id: reader.read_string()?,
            nonce: read_fixed_bytes(reader, "nonce")?,
        }),
        ENCRYPTION_PASSWORD_CHACHA20_POLY1305 => Ok(EncryptionManifest::PasswordChaCha20Poly1305 {
            key_id: reader.read_string()?,
            nonce: read_fixed_bytes(reader, "nonce")?,
            salt: read_fixed_bytes(reader, "salt")?,
            params: read_password_params(reader)?,
        }),
        tag => Err(SnapshotCodecError::Decode(format!(
            "invalid encryption tag {tag}"
        ))),
    }
}

fn write_password_params(out: &mut Vec<u8>, params: PasswordKdfParams) {
    write_u32(out, params.memory_cost_kib);
    write_u32(out, params.time_cost);
    write_u32(out, params.parallelism);
}

fn read_password_params(reader: &mut BodyReader<'_>) -> Result<PasswordKdfParams> {
    Ok(PasswordKdfParams {
        memory_cost_kib: reader.read_u32()?,
        time_cost: reader.read_u32()?,
        parallelism: reader.read_u32()?,
    })
}

fn read_fixed_bytes<const N: usize>(reader: &mut BodyReader<'_>, field: &str) -> Result<[u8; N]> {
    let bytes = reader.read_bytes()?;
    bytes.try_into().map_err(|_| {
        SnapshotCodecError::Decode(format!(
            "invalid {field} length: expected {N}, got {}",
            bytes.len()
        ))
    })
}

fn read_header_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| SnapshotCodecError::Decode("snapshot header offset overflow".into()))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| SnapshotCodecError::Decode("truncated snapshot header".into()))?
        .try_into()
        .map_err(|_| SnapshotCodecError::Decode("truncated snapshot header".into()))
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
