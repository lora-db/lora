//! JSON option / credential adapters used by the language bindings.
//!
//! The HTTP, WASM, FFI, and node bindings all hand snapshot save and
//! load options across the FFI boundary as `serde_json::Value`. This
//! module owns the validation and shape normalization so the rest of
//! the database can take typed [`SnapshotOptions`] / [`SnapshotCredentials`]
//! values directly.
//!
//! Two public entry points sit at the top:
//!
//! * [`snapshot_options_from_json`] — build save options.
//! * [`snapshot_credentials_from_json`] — build load credentials.
//!
//! All other helpers (compression / encryption / KDF / field-extraction)
//! are private — bindings consume only the two public entry points.

use anyhow::{anyhow, Result};

use lora_snapshot::{
    Compression, EncryptionKey, PasswordKdfParams, SnapshotCredentials, SnapshotEncryption,
    SnapshotOptions, SnapshotPassword,
};

const DEFAULT_SNAPSHOT_KEY_ID: &str = "default";

/// Build snapshot save options from the JSON shape used by the language
/// bindings.
///
/// Supported shape:
///
/// `{ compression?: "none" | "gzip" | { format: "gzip", level?: number },
///    encryption?: { type: "password", keyId?: string, password: string,
///                   params?: { memoryCostKib?: number, timeCost?: number,
///                              parallelism?: number } } }`
///
/// `encryption` may also be a raw 32-byte key object with
/// `{ type: "key", keyId?: string, key: number[] }`.
pub fn snapshot_options_from_json(value: Option<serde_json::Value>) -> Result<SnapshotOptions> {
    let Some(value) = value else {
        return Ok(SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        });
    };
    if value.is_null() {
        return Ok(SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        });
    }

    let compression = match value.get("compression") {
        Some(value) => parse_snapshot_compression_json(value)?,
        None => Compression::None,
    };
    let encryption = parse_snapshot_credentials_json(Some(value))?;

    Ok(SnapshotOptions {
        compression,
        encryption,
    })
}

/// Build snapshot load credentials from the JSON shape used by the language
/// bindings. The credential object may be supplied directly, or under
/// `credentials` / `encryption` so the same options object can be reused for
/// save and load.
pub fn snapshot_credentials_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<SnapshotCredentials>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_snapshot_credentials_json(Some(value))
}

fn parse_snapshot_compression_json(value: &serde_json::Value) -> Result<Compression> {
    match value {
        serde_json::Value::Null => Ok(Compression::None),
        serde_json::Value::String(format) => snapshot_compression_from_parts(format, None),
        serde_json::Value::Object(obj) => {
            let format =
                string_field(obj, &["format", "type"])?.unwrap_or_else(|| "none".to_string());
            let level = u32_field(obj, &["level"])?;
            snapshot_compression_from_parts(&format, level)
        }
        _ => Err(anyhow!(
            "snapshot compression must be a string or object with a format field"
        )),
    }
}

fn snapshot_compression_from_parts(format: &str, level: Option<u32>) -> Result<Compression> {
    match format {
        "none" | "identity" | "uncompressed" => Ok(Compression::None),
        "gzip" => {
            let level = level.unwrap_or(1);
            if level > 9 {
                return Err(anyhow!(
                    "gzip snapshot compression level must be between 0 and 9"
                ));
            }
            Ok(Compression::Gzip { level })
        }
        other => Err(anyhow!("unknown snapshot compression '{other}'")),
    }
}

fn parse_snapshot_credentials_json(
    value: Option<serde_json::Value>,
) -> Result<Option<SnapshotCredentials>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let credential_value = if let Some(credentials) = value.get("credentials") {
        credentials
    } else if let Some(encryption) = value.get("encryption") {
        encryption
    } else if looks_like_snapshot_encryption(&value) {
        &value
    } else {
        return Ok(None);
    };

    if credential_value.is_null() {
        return Ok(None);
    }

    Ok(Some(parse_snapshot_encryption_json(credential_value)?))
}

fn looks_like_snapshot_encryption(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("password")
        || obj.contains_key("key")
        || obj.contains_key("keyBytes")
        || obj.contains_key("key_bytes")
}

fn parse_snapshot_encryption_json(value: &serde_json::Value) -> Result<SnapshotEncryption> {
    let serde_json::Value::Object(obj) = value else {
        return Err(anyhow!("snapshot encryption must be an object"));
    };

    let kind = string_field(obj, &["type", "kind"])?.unwrap_or_else(|| {
        if obj.contains_key("key") || obj.contains_key("keyBytes") || obj.contains_key("key_bytes")
        {
            "key".to_string()
        } else {
            "password".to_string()
        }
    });

    match kind.as_str() {
        "password" | "passphrase" => {
            let key_id = string_field(obj, &["keyId", "key_id"])?
                .unwrap_or_else(|| DEFAULT_SNAPSHOT_KEY_ID.to_string());
            let password = required_string_field(obj, &["password"])?;
            let params = parse_password_kdf_params(
                obj.get("params")
                    .or_else(|| obj.get("kdfParams"))
                    .or_else(|| obj.get("kdf_params")),
            )?;
            Ok(SnapshotEncryption::Password(SnapshotPassword::with_params(
                key_id, password, params,
            )))
        }
        "key" | "raw_key" | "rawKey" => {
            let key_id = string_field(obj, &["keyId", "key_id"])?
                .unwrap_or_else(|| DEFAULT_SNAPSHOT_KEY_ID.to_string());
            let key = required_key_field(obj, &["key", "keyBytes", "key_bytes"])?;
            Ok(SnapshotEncryption::Key(EncryptionKey::new(key_id, key)))
        }
        other => Err(anyhow!("unknown snapshot encryption type '{other}'")),
    }
}

fn parse_password_kdf_params(value: Option<&serde_json::Value>) -> Result<PasswordKdfParams> {
    let Some(value) = value else {
        return Ok(PasswordKdfParams::interactive());
    };
    if value.is_null() {
        return Ok(PasswordKdfParams::interactive());
    }
    let serde_json::Value::Object(obj) = value else {
        return Err(anyhow!("snapshot password params must be an object"));
    };

    let defaults = PasswordKdfParams::interactive();
    let memory_cost_kib =
        u32_field(obj, &["memoryCostKib", "memory_cost_kib"])?.unwrap_or(defaults.memory_cost_kib);
    let time_cost = u32_field(obj, &["timeCost", "time_cost"])?.unwrap_or(defaults.time_cost);
    let parallelism = u32_field(obj, &["parallelism"])?.unwrap_or(defaults.parallelism);

    if memory_cost_kib == 0 || time_cost == 0 || parallelism == 0 {
        return Err(anyhow!(
            "snapshot password params memoryCostKib, timeCost, and parallelism must be greater than zero"
        ));
    }

    Ok(PasswordKdfParams {
        memory_cost_kib,
        time_cost,
        parallelism,
    })
}

fn string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<Option<String>> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return match value {
                serde_json::Value::Null => Ok(None),
                serde_json::Value::String(value) => Ok(Some(value.clone())),
                _ => Err(anyhow!("snapshot field '{name}' must be a string")),
            };
        }
    }
    Ok(None)
}

fn required_string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<String> {
    string_field(obj, names)?.ok_or_else(|| anyhow!("snapshot field '{}' is required", names[0]))
}

fn u32_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<Option<u32>> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return match value {
                serde_json::Value::Null => Ok(None),
                serde_json::Value::Number(number) => {
                    let Some(value) = number.as_u64() else {
                        return Err(anyhow!(
                            "snapshot field '{name}' must be a non-negative integer"
                        ));
                    };
                    if value > u32::MAX as u64 {
                        return Err(anyhow!("snapshot field '{name}' is too large"));
                    }
                    Ok(Some(value as u32))
                }
                _ => Err(anyhow!("snapshot field '{name}' must be an integer")),
            };
        }
    }
    Ok(None)
}

fn required_key_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Result<[u8; 32]> {
    for name in names {
        if let Some(value) = obj.get(*name) {
            return parse_key_bytes(value, name);
        }
    }
    Err(anyhow!("snapshot field '{}' is required", names[0]))
}

fn parse_key_bytes(value: &serde_json::Value, field_name: &str) -> Result<[u8; 32]> {
    let serde_json::Value::Array(values) = value else {
        return Err(anyhow!(
            "snapshot field '{field_name}' must be an array of 32 byte values"
        ));
    };
    if values.len() != 32 {
        return Err(anyhow!(
            "snapshot field '{field_name}' must contain exactly 32 byte values"
        ));
    }

    let mut out = [0u8; 32];
    for (idx, value) in values.iter().enumerate() {
        let Some(byte) = value.as_u64() else {
            return Err(anyhow!(
                "snapshot field '{field_name}' item {idx} must be a byte integer"
            ));
        };
        if byte > u8::MAX as u64 {
            return Err(anyhow!(
                "snapshot field '{field_name}' item {idx} must be between 0 and 255"
            ));
        }
        out[idx] = byte as u8;
    }
    Ok(out)
}
