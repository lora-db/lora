//! Lora portable container codec for named `.loradb` databases.
//!
//! The file is a small Lora-owned envelope followed by length-prefixed frames.
//! Named databases can carry a base snapshot frame plus WAL delta frames. The
//! runtime store remains in-memory; this container shape is the bridge toward a
//! future paged/containerized store.
//!
//! The snapshot frame is copied from and to `snapshot.lsnap` in the work
//! directory as a stream: it is the size of the graph's encoding, and
//! neither writing nor opening a container holds it in memory. Its frame is
//! gzip-compressed again on the way: the snapshot's own fast deflate leaves
//! long repeats in its columns that a second pass halves, and the second
//! pass reads only the compressed snapshot.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzipCompression;
use lora_wal::WalError;

use super::platform::{replace_file_atomic, sync_dir};
use super::workspace::{make_archive_tmp_path, sorted_wal_files};
use crate::durable_io::sync_file;

pub(crate) const CONTAINER_MAGIC: &[u8; 8] = b"LORADB2\0";

const CONTAINER_VERSION: u32 = 1;
const HEADER_LEN: usize = 32;
const FRAME_MAGIC: &[u8; 8] = b"LORAFRM\0";
const FRAME_HEADER_LEN: usize = 48;
const FRAME_KIND_WAL_FILE: u8 = 1;
const FRAME_KIND_SNAPSHOT: u8 = 2;
const SNAPSHOT_FRAME_NAME: &str = "snapshot.lsnap";
/// The snapshot's file in the work directory, named as its frame.
pub(super) const SNAPSHOT_FILE_NAME: &str = SNAPSHOT_FRAME_NAME;
/// Where a snapshot is written before it replaces [`SNAPSHOT_FILE_NAME`].
/// The WAL ignores both: it lists only `*.wal`.
pub(super) const STAGED_SNAPSHOT_FILE_NAME: &str = "snapshot.lsnap.tmp";
const FLAG_COMPRESSED: u8 = 1 << 0;
const FLAG_ENCRYPTED: u8 = 1 << 1;

const CODEC_PLAIN: u8 = 0;
const CODEC_GZIP: u8 = 1;
const ENCRYPTION_NONE: u8 = 0;
const ENCRYPTION_CHACHA20_POLY1305: u8 = 1;

#[derive(Clone, Default)]
pub(super) struct ArchiveCodec {
    encryption_key: Option<[u8; 32]>,
}

#[allow(dead_code)]
impl ArchiveCodec {
    pub(super) fn encrypted(key: [u8; 32]) -> Self {
        Self {
            encryption_key: Some(key),
        }
    }

    fn encode_body(&self, name: &str, raw: Vec<u8>) -> Result<EncodedBody, WalError> {
        let raw_len = raw.len() as u64;
        let compressed = gzip_compress(&raw)?;
        let (mut body, codec, mut flags) = if compressed.len() < raw.len() {
            (compressed, CODEC_GZIP, FLAG_COMPRESSED)
        } else {
            (raw, CODEC_PLAIN, 0)
        };

        let mut nonce = [0u8; 12];
        let encryption = if let Some(key) = self.encryption_key {
            getrandom::getrandom(&mut nonce).map_err(|e| {
                WalError::Malformed(format!("database container nonce generation failed: {e}"))
            })?;
            body = encrypt_body(&body, &key, &nonce, name.as_bytes())?;
            flags |= FLAG_ENCRYPTED;
            ENCRYPTION_CHACHA20_POLY1305
        } else {
            ENCRYPTION_NONE
        };

        Ok(EncodedBody {
            body,
            raw_len,
            flags,
            codec,
            encryption,
            nonce,
        })
    }

    fn decode_body(
        &self,
        name: &str,
        header: &FrameHeader,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, WalError> {
        let encrypted = header.flags & FLAG_ENCRYPTED != 0;
        let compressed = header.flags & FLAG_COMPRESSED != 0;

        let mut body = if encrypted {
            let Some(key) = self.encryption_key else {
                return Err(WalError::Malformed(
                    "database container frame is encrypted but no key was configured".into(),
                ));
            };
            if header.encryption != ENCRYPTION_CHACHA20_POLY1305 {
                return Err(WalError::Malformed(format!(
                    "unsupported database container encryption codec {}",
                    header.encryption
                )));
            }
            decrypt_body(&body, &key, &header.nonce, name.as_bytes())?
        } else {
            if header.encryption != ENCRYPTION_NONE {
                return Err(WalError::Malformed(
                    "database container frame declares encryption without encrypted flag".into(),
                ));
            }
            body
        };

        body = if compressed {
            if header.codec != CODEC_GZIP {
                return Err(WalError::Malformed(format!(
                    "unsupported database container compression codec {}",
                    header.codec
                )));
            }
            gzip_decompress(&body)?
        } else {
            if header.codec != CODEC_PLAIN {
                return Err(WalError::Malformed(
                    "database container frame declares compression codec without compressed flag"
                        .into(),
                ));
            }
            body
        };

        if body.len() as u64 != header.raw_len {
            return Err(WalError::Malformed(format!(
                "database container frame {} decoded to {} bytes, expected {}",
                name,
                body.len(),
                header.raw_len
            )));
        }

        Ok(body)
    }
}

struct EncodedBody {
    body: Vec<u8>,
    raw_len: u64,
    flags: u8,
    codec: u8,
    encryption: u8,
    nonce: [u8; 12],
}

#[derive(Debug)]
struct FrameHeader {
    kind: u8,
    flags: u8,
    codec: u8,
    encryption: u8,
    name_len: u16,
    body_len: u64,
    raw_len: u64,
    crc32: u32,
    nonce: [u8; 12],
}

pub(super) fn write_archive_atomic(
    wal_dir: &Path,
    archive_path: &Path,
    max_archive_bytes: u64,
) -> Result<(), WalError> {
    let tmp_path = make_archive_tmp_path(archive_path);
    let result = write_archive_tmp(wal_dir, &tmp_path, &ArchiveCodec::default()).and_then(|_| {
        let len = fs::metadata(&tmp_path)?.len();
        if len > max_archive_bytes {
            let _ = fs::remove_file(&tmp_path);
            return Err(WalError::Malformed(format!(
                "database container {} would be {} bytes, above configured limit {}",
                archive_path.display(),
                len,
                max_archive_bytes
            )));
        }
        replace_file_atomic(&tmp_path, archive_path)?;
        if let Some(parent) = archive_path.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_archive_tmp(
    wal_dir: &Path,
    tmp_path: &Path,
    codec: &ArchiveCodec,
) -> Result<(), WalError> {
    {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp_path)?;
        let mut writer = BufWriter::new(file);
        write_container_header(&mut writer)?;

        match File::open(wal_dir.join(SNAPSHOT_FILE_NAME)) {
            Ok(snapshot) => write_compressed_file_frame(
                &mut writer,
                FRAME_KIND_SNAPSHOT,
                SNAPSHOT_FRAME_NAME,
                snapshot,
            )?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        for entry in sorted_wal_files(wal_dir)? {
            let name = entry
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| WalError::Malformed("WAL file name is not UTF-8".into()))?;
            if !is_safe_wal_file_name(name) {
                return Err(WalError::Malformed(format!(
                    "unsafe WAL container entry name: {name}"
                )));
            }
            let raw = fs::read(&entry)?;
            write_frame(&mut writer, codec, FRAME_KIND_WAL_FILE, name, raw)?;
        }

        writer.flush()?;
        let file = writer
            .into_inner()
            .map_err(|e| WalError::Io(e.into_error()))?;
        sync_file(&file)?;
    }
    Ok(())
}

/// Extract the frames of `archive_path` into `dest_dir`: the snapshot to
/// [`SNAPSHOT_FILE_NAME`] and, when `wal` is set, each WAL segment under its
/// own name. Every frame streams through, so memory stays bounded by the
/// copy buffers whatever the frame's size. Returns whether the container
/// held a snapshot.
pub(super) fn extract_archive(
    archive_path: &Path,
    dest_dir: &Path,
    wal: bool,
) -> Result<bool, WalError> {
    let codec = ArchiveCodec::default();
    let file = File::open(archive_path)?;
    let mut reader = BufReader::new(file);
    read_container_header(&mut reader)?;

    let mut saw_wal = false;
    let mut saw_snapshot = false;
    while let Some(header) = read_frame_header(&mut reader)? {
        let name = read_exact_vec(&mut reader, u64::from(header.name_len), "frame name")?;
        let name = String::from_utf8(name).map_err(|_| {
            WalError::Malformed("database container frame name is not UTF-8".into())
        })?;

        match header.kind {
            FRAME_KIND_WAL_FILE => {
                if !is_safe_wal_file_name(&name) {
                    return Err(WalError::Malformed(format!(
                        "unsafe WAL container entry name: {name}"
                    )));
                }
                if wal {
                    let path = dest_dir.join(&name);
                    let mut out = OpenOptions::new().write(true).create_new(true).open(path)?;
                    extract_frame_body(&mut reader, &codec, &header, &name, &mut out)?;
                    sync_file(&out)?;
                } else {
                    extract_frame_body(&mut reader, &codec, &header, &name, &mut io::sink())?;
                }
                saw_wal = true;
            }
            FRAME_KIND_SNAPSHOT => {
                if name != SNAPSHOT_FRAME_NAME {
                    return Err(WalError::Malformed(format!(
                        "unexpected snapshot frame name: {name}"
                    )));
                }
                let path = dest_dir.join(SNAPSHOT_FILE_NAME);
                let mut out = BufWriter::new(
                    OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(path)?,
                );
                extract_frame_body(&mut reader, &codec, &header, &name, &mut out)?;
                let out = out.into_inner().map_err(|e| WalError::Io(e.into_error()))?;
                sync_file(&out)?;
                saw_snapshot = true;
            }
            other => {
                return Err(WalError::Malformed(format!(
                    "unsupported database container frame kind {other}"
                )));
            }
        }
    }

    if !saw_wal && !saw_snapshot {
        return Err(WalError::Malformed(
            "database container does not contain snapshot or WAL frames".into(),
        ));
    }
    Ok(saw_snapshot)
}

/// Decode one frame body from `reader` into `out`, checking its checksum
/// and decoded length. A plain or compressed body streams; an encrypted
/// one is sealed whole, so it is read into memory to open.
fn extract_frame_body(
    reader: &mut impl Read,
    codec: &ArchiveCodec,
    header: &FrameHeader,
    name: &str,
    out: &mut impl Write,
) -> Result<(), WalError> {
    if header.flags & FLAG_ENCRYPTED != 0 {
        let body = read_exact_vec(reader, header.body_len, "frame body")?;
        validate_frame_crc(header, name.as_bytes(), &body)?;
        let raw = codec.decode_body(name, header, body)?;
        out.write_all(&raw)?;
        return Ok(());
    }
    if header.encryption != ENCRYPTION_NONE {
        return Err(WalError::Malformed(
            "database container frame declares encryption without encrypted flag".into(),
        ));
    }
    let compressed = header.flags & FLAG_COMPRESSED != 0;
    if compressed && header.codec != CODEC_GZIP {
        return Err(WalError::Malformed(format!(
            "unsupported database container compression codec {}",
            header.codec
        )));
    }
    if !compressed && header.codec != CODEC_PLAIN {
        return Err(WalError::Malformed(
            "database container frame declares compression codec without compressed flag".into(),
        ));
    }

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(name.as_bytes());
    let mut body = CrcReader {
        inner: reader.take(header.body_len),
        hasher: &mut hasher,
        read: 0,
    };
    let raw_len = if compressed {
        let mut decoder = GzDecoder::new(&mut body);
        let raw_len = io::copy(&mut decoder, out)?;
        // The checksum covers any bytes the decoder left unread.
        io::copy(decoder.into_inner(), &mut io::sink())?;
        raw_len
    } else {
        io::copy(&mut body, out)?
    };
    if body.read != header.body_len {
        return Err(WalError::Malformed(
            "database container frame body is truncated".into(),
        ));
    }
    if hasher.finalize() != header.crc32 {
        return Err(WalError::Malformed(
            "database container frame checksum mismatch".into(),
        ));
    }
    if raw_len != header.raw_len {
        return Err(WalError::Malformed(format!(
            "database container frame {name} decoded to {raw_len} bytes, expected {}",
            header.raw_len
        )));
    }
    Ok(())
}

struct CrcReader<'h, R> {
    inner: R,
    hasher: &'h mut crc32fast::Hasher,
    read: u64,
}

impl<R: Read> Read for CrcReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// Write `file` as one gzip-compressed frame without holding it. The
/// header carries the compressed length and checksum, so it is written
/// last, over a placeholder.
fn write_compressed_file_frame<W: Write + Seek>(
    out: &mut W,
    kind: u8,
    name: &str,
    file: File,
) -> Result<(), WalError> {
    let name_bytes = name.as_bytes();
    let header_at = out.stream_position()?;
    out.write_all(&[0u8; FRAME_HEADER_LEN])?;
    out.write_all(name_bytes)?;

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(name_bytes);
    let (raw_len, body_len) = {
        let mut body = CrcWriter {
            inner: &mut *out,
            hasher: &mut hasher,
            written: 0,
        };
        let mut encoder = GzEncoder::new(&mut body, GzipCompression::new(1));
        let raw_len = io::copy(&mut BufReader::new(file), &mut encoder)?;
        encoder.finish()?;
        (raw_len, body.written)
    };
    let crc32 = hasher.finalize();
    let end = out.stream_position()?;

    out.seek(SeekFrom::Start(header_at))?;
    out.write_all(FRAME_MAGIC)?;
    out.write_all(&[kind, FLAG_COMPRESSED, CODEC_GZIP, ENCRYPTION_NONE])?;
    out.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?; // reserved
    out.write_all(&body_len.to_le_bytes())?;
    out.write_all(&raw_len.to_le_bytes())?;
    out.write_all(&crc32.to_le_bytes())?;
    out.write_all(&[0u8; 12])?;
    out.seek(SeekFrom::Start(end))?;
    Ok(())
}

struct CrcWriter<'h, W> {
    inner: W,
    hasher: &'h mut crc32fast::Hasher,
    written: u64,
}

impl<W: Write> Write for CrcWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn read_exact_vec(input: &mut impl Read, len: u64, field: &str) -> Result<Vec<u8>, WalError> {
    let len = usize::try_from(len)
        .map_err(|_| WalError::Malformed(format!("database container {field} is too large")))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| {
        WalError::Malformed(format!(
            "database container {field} is too large to allocate"
        ))
    })?;
    bytes.resize(len, 0);
    input.read_exact(&mut bytes).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            WalError::Malformed(format!("database container {field} is truncated"))
        } else {
            WalError::Io(e)
        }
    })?;
    Ok(bytes)
}

fn write_container_header(out: &mut impl Write) -> Result<(), WalError> {
    out.write_all(CONTAINER_MAGIC)?;
    out.write_all(&CONTAINER_VERSION.to_le_bytes())?;
    out.write_all(&0u32.to_le_bytes())?; // flags
    out.write_all(&0u64.to_le_bytes())?; // reserved
    out.write_all(&0u64.to_le_bytes())?; // reserved
    Ok(())
}

fn read_container_header(input: &mut impl Read) -> Result<(), WalError> {
    let mut header = [0u8; HEADER_LEN];
    input.read_exact(&mut header).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            WalError::Malformed("database container header is truncated".into())
        } else {
            WalError::Io(e)
        }
    })?;
    if &header[..8] != CONTAINER_MAGIC {
        return Err(WalError::Malformed(
            "database container has invalid magic".into(),
        ));
    }
    let version = read_u32_field(&header, 8, "container version")?;
    if version != CONTAINER_VERSION {
        return Err(WalError::Malformed(format!(
            "unsupported database container version {version}"
        )));
    }
    let flags = read_u32_field(&header, 12, "container flags")?;
    if flags != 0 {
        return Err(WalError::Malformed(format!(
            "unsupported database container flags {flags}"
        )));
    }
    Ok(())
}

fn write_frame(
    out: &mut impl Write,
    codec: &ArchiveCodec,
    kind: u8,
    name: &str,
    raw: Vec<u8>,
) -> Result<(), WalError> {
    let encoded = codec.encode_body(name, raw)?;
    let name_bytes = name.as_bytes();
    if name_bytes.len() > u16::MAX as usize {
        return Err(WalError::Malformed(format!(
            "database container frame name too long: {name}"
        )));
    }

    let crc32 = frame_crc(name_bytes, &encoded.body);
    out.write_all(FRAME_MAGIC)?;
    out.write_all(&[kind, encoded.flags, encoded.codec, encoded.encryption])?;
    out.write_all(&(name_bytes.len() as u16).to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?; // reserved
    out.write_all(&(encoded.body.len() as u64).to_le_bytes())?;
    out.write_all(&encoded.raw_len.to_le_bytes())?;
    out.write_all(&crc32.to_le_bytes())?;
    out.write_all(&encoded.nonce)?;
    out.write_all(name_bytes)?;
    out.write_all(&encoded.body)?;
    Ok(())
}

fn read_frame_header(input: &mut impl Read) -> Result<Option<FrameHeader>, WalError> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(WalError::Io(e)),
    }
    if &header[..8] != FRAME_MAGIC {
        return Err(WalError::Malformed(
            "database container frame has invalid magic".into(),
        ));
    }
    let name_len = read_u16_field(&header, 12, "frame name length")?;
    let reserved = read_u16_field(&header, 14, "frame reserved")?;
    if reserved != 0 {
        return Err(WalError::Malformed(
            "database container frame reserved bytes are non-zero".into(),
        ));
    }
    let body_len = read_u64_field(&header, 16, "frame body length")?;
    let raw_len = read_u64_field(&header, 24, "frame raw length")?;
    let crc32 = read_u32_field(&header, 32, "frame checksum")?;
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&header[36..48]);
    Ok(Some(FrameHeader {
        kind: header[8],
        flags: header[9],
        codec: header[10],
        encryption: header[11],
        name_len,
        body_len,
        raw_len,
        crc32,
        nonce,
    }))
}

fn read_u16_field(bytes: &[u8], start: usize, field: &str) -> Result<u16, WalError> {
    Ok(u16::from_le_bytes(read_fixed_field(bytes, start, field)?))
}

fn read_u32_field(bytes: &[u8], start: usize, field: &str) -> Result<u32, WalError> {
    Ok(u32::from_le_bytes(read_fixed_field(bytes, start, field)?))
}

fn read_u64_field(bytes: &[u8], start: usize, field: &str) -> Result<u64, WalError> {
    Ok(u64::from_le_bytes(read_fixed_field(bytes, start, field)?))
}

fn read_fixed_field<const N: usize>(
    bytes: &[u8],
    start: usize,
    field: &str,
) -> Result<[u8; N], WalError> {
    let end = start.checked_add(N).ok_or_else(|| {
        WalError::Malformed(format!("database container {field} offset overflow"))
    })?;
    bytes
        .get(start..end)
        .and_then(|field_bytes| field_bytes.try_into().ok())
        .ok_or_else(|| WalError::Malformed(format!("database container {field} is truncated")))
}

fn validate_frame_crc(header: &FrameHeader, name: &[u8], body: &[u8]) -> Result<(), WalError> {
    let actual = frame_crc(name, body);
    if actual != header.crc32 {
        return Err(WalError::Malformed(
            "database container frame checksum mismatch".into(),
        ));
    }
    Ok(())
}

fn frame_crc(name: &[u8], body: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(name);
    hasher.update(body);
    hasher.finalize()
}

fn gzip_compress(bytes: &[u8]) -> Result<Vec<u8>, WalError> {
    let mut encoder = GzEncoder::new(Vec::new(), GzipCompression::new(1));
    encoder.write_all(bytes)?;
    encoder.finish().map_err(WalError::Io)
}

fn gzip_decompress(bytes: &[u8]) -> Result<Vec<u8>, WalError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

fn encrypt_body(
    bytes: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
) -> Result<Vec<u8>, WalError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: bytes, aad })
        .map_err(|_| WalError::Malformed("database container encryption failed".into()))
}

fn decrypt_body(
    bytes: &[u8],
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
) -> Result<Vec<u8>, WalError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: bytes, aad })
        .map_err(|_| WalError::Malformed("database container decryption failed".into()))
}

fn is_safe_wal_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".wal") else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lora-archive-format-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn container(frames: &[(u8, &str, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        write_container_header(&mut out).unwrap();
        for (kind, name, raw) in frames {
            write_frame(&mut out, &ArchiveCodec::default(), *kind, name, raw.clone()).unwrap();
        }
        out
    }

    #[test]
    fn extract_streams_a_compressed_snapshot_frame_written_by_earlier_versions() {
        let dir = scratch("compressed");
        let snapshot = b"snapshot bytes ".repeat(10_000);
        let wal = b"wal".repeat(100);
        let bytes = container(&[
            (FRAME_KIND_SNAPSHOT, SNAPSHOT_FRAME_NAME, snapshot.clone()),
            (FRAME_KIND_WAL_FILE, "0000000001.wal", wal.clone()),
        ]);
        let archive = dir.join("app.loradb");
        fs::write(&archive, &bytes).unwrap();
        let out = dir.join("out");
        fs::create_dir(&out).unwrap();

        assert!(extract_archive(&archive, &out, true).unwrap());
        assert_eq!(fs::read(out.join(SNAPSHOT_FILE_NAME)).unwrap(), snapshot);
        assert_eq!(fs::read(out.join("0000000001.wal")).unwrap(), wal);

        // Rewritten as a stream, the snapshot frame round-trips.
        write_archive_atomic(&out, &archive, u64::MAX).unwrap();
        let again = dir.join("again");
        fs::create_dir(&again).unwrap();
        assert!(extract_archive(&archive, &again, false).unwrap());
        assert_eq!(fs::read(again.join(SNAPSHOT_FILE_NAME)).unwrap(), snapshot);
        assert!(!again.join("0000000001.wal").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_rejects_a_corrupt_snapshot_frame() {
        let dir = scratch("corrupt");
        let archive = dir.join("app.loradb");
        let mut bytes = container(&[(FRAME_KIND_SNAPSHOT, SNAPSHOT_FRAME_NAME, vec![7; 4096])]);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&archive, &bytes).unwrap();
        let err = extract_archive(&archive, &dir, false).unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");

        bytes.truncate(bytes.len() - 10);
        fs::write(&archive, &bytes).unwrap();
        assert!(extract_archive(&archive, &dir, false).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codec_encrypts_and_decrypts_body() {
        let codec = ArchiveCodec::encrypted([7; 32]);
        let encoded = codec
            .encode_body("0000000001.wal", b"hello portable lora".to_vec())
            .unwrap();
        assert_ne!(encoded.body, b"hello portable lora");
        let header = FrameHeader {
            kind: FRAME_KIND_WAL_FILE,
            flags: encoded.flags,
            codec: encoded.codec,
            encryption: encoded.encryption,
            name_len: "0000000001.wal".len() as u16,
            body_len: encoded.body.len() as u64,
            raw_len: encoded.raw_len,
            crc32: frame_crc(b"0000000001.wal", &encoded.body),
            nonce: encoded.nonce,
        };
        let decoded = codec
            .decode_body("0000000001.wal", &header, encoded.body)
            .unwrap();
        assert_eq!(decoded, b"hello portable lora");
    }
}
