use std::io::{Cursor, Read, Write};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzipCompression;

use crate::error::{Result, SnapshotCodecError};
use crate::options::{Compression, PasswordKdfParams};
use crate::MAGIC;

pub(crate) fn compress(mut bytes: Vec<u8>, compression: Compression) -> Result<Vec<u8>> {
    match compression {
        Compression::None => {
            bytes.shrink_to_fit();
            Ok(bytes)
        }
        Compression::Gzip { level } => {
            let mut encoder = GzEncoder::new(Vec::new(), GzipCompression::new(level.min(9)));
            encoder.write_all(&bytes)?;
            Ok(encoder.finish()?)
        }
    }
}

pub(crate) fn decompress(bytes: Vec<u8>, compression: Compression) -> Result<Vec<u8>> {
    match compression {
        Compression::None => Ok(bytes),
        Compression::Gzip { .. } => {
            let mut decoder = GzDecoder::new(Cursor::new(bytes));
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            Ok(out)
        }
    }
}

pub(crate) fn derive_password_key(
    password: &[u8],
    salt: &[u8; 16],
    params: PasswordKdfParams,
) -> Result<[u8; 32]> {
    let params = Params::new(
        params.memory_cost_kib,
        params.time_cost,
        params.parallelism,
        Some(32),
    )
    .map_err(|e| SnapshotCodecError::PasswordKdf(e.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(password, salt, &mut out)
        .map_err(|e| SnapshotCodecError::PasswordKdf(e.to_string()))?;
    Ok(out)
}

pub(crate) fn encrypt_body(bytes: &[u8], key: &[u8; 32], nonce: &[u8; 12]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: bytes,
                aad: MAGIC,
            },
        )
        .map_err(|_| SnapshotCodecError::Encrypt)
}

pub(crate) fn decrypt_body(bytes: &[u8], key: &[u8; 32], nonce: &[u8; 12]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: bytes,
                aad: MAGIC,
            },
        )
        .map_err(|_| SnapshotCodecError::Decrypt)
}
