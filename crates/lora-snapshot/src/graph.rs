//! Streaming snapshot of an [`InMemoryGraph`].
//!
//! [`crate::write_snapshot`] and [`crate::read_snapshot`] go through a
//! [`SnapshotPayload`](lora_store::SnapshotPayload): every record decoded
//! into an owned copy, then the columns, then the body, then the envelope,
//! each held whole beside the graph. For a graph of a few gigabytes that
//! is several times the graph again. This module writes and reads the same
//! bytes without those copies.
//!
//! The body is column-major, so the encoder walks the graph once per
//! column through borrowed views and writes each column as it goes. Only
//! the dictionaries (labels, relationship types, property keys) are held.
//!
//! The decoder keeps what it needs to place each value on its owner: the
//! node and relationship columns (under 40 bytes an entity, with its
//! property count) and the key of every property (1 to 4 bytes each, by
//! the size of the key dictionary). Values arrive in owner order, so each
//! record is built, handed to a [`SnapshotGraphBuilder`] and dropped. A
//! snapshot whose properties are not in owner order (no encoder writes
//! one) falls back to holding every record until the values are read.
//!
//! The envelope's checksum sits in the header and covers the manifest,
//! which holds the body length, so a writer cannot know it before the
//! body is written. [`write_graph_snapshot`] writes the body, then seeks
//! back to fill in the header and reads the body back to hash it.
//! Encryption seals the whole body, so an encrypted snapshot is built in
//! memory (compressed, without the record copies).

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzipCompression;
use lora_store::codec::{
    decode_constraint_definitions, decode_index_definitions, decode_property_value,
    encode_constraint_definitions, encode_index_definitions, encode_property_value,
};
use lora_store::{
    ConstraintDefinition, InMemoryGraph, IndexDefinition, Labels, Name, NodeRecord, Properties,
    PropertyValue, RelationshipRecord, SnapshotCatalogs, SnapshotGraphBuilder, ValueRef,
    VectorIndexSnapshot,
};

use crate::codec::{decrypt_snapshot_body, encrypt_snapshot_body, SnapshotInfo};
use crate::envelope::{
    decode_manifest, encode_envelope, encode_manifest, manifest_info, EncryptionManifest, Manifest,
};
use crate::errors::{Result, SnapshotCodecError};
use crate::format::{
    nested_decode_depth, nested_encode_depth, BODY_FORMAT_VERSION, BODY_FORMAT_VERSION_V2,
    BODY_FORMAT_VERSION_V3, BODY_FORMAT_VERSION_V4, FORMAT_VERSION, HEADER_LEN, MAGIC,
};
use crate::options::{Compression, SnapshotCredentials, SnapshotOptions};

/// Largest allocation made up front for a length read from the body. A
/// longer field grows as its bytes arrive, so a corrupt length fails on
/// the truncated stream instead of reserving memory it names.
const MAX_PREALLOC: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Write a snapshot of `graph` at the current position of `file`, which
/// must be readable, writable and seekable (a [`std::fs::File`] opened
/// read-write). Produces the same bytes [`crate::write_snapshot`] would for
/// the graph's payload, without the payload. Leaves `file` positioned at
/// the end of the snapshot.
pub fn write_graph_snapshot<F: Read + Write + Seek>(
    file: &mut F,
    graph: &InMemoryGraph,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<SnapshotInfo> {
    if options.encryption.is_some() {
        let (bytes, info) = encode_graph_snapshot(graph, wal_lsn, options)?;
        file.write_all(&bytes)?;
        return Ok(info);
    }

    let start = file.stream_position()?;
    let placeholder = Manifest {
        format_version: FORMAT_VERSION,
        wal_lsn,
        node_count: 0,
        relationship_count: 0,
        compression: options.compression,
        encryption: EncryptionManifest::None,
        body_len: 0,
    };
    let placeholder_len = encode_manifest(&placeholder)?.len();
    file.write_all(&[0u8; HEADER_LEN])?;
    file.write_all(&vec![0u8; placeholder_len])?;

    let (counts, body_len) = {
        let mut counted = CountingWriter::new(BufWriter::new(&mut *file));
        let counts = write_compressed_body(&mut counted, graph, options.compression)?;
        counted.flush()?;
        (counts, counted.written)
    };

    let manifest = Manifest {
        node_count: counts.nodes,
        relationship_count: counts.relationships,
        body_len,
        ..placeholder
    };
    let manifest_bytes = encode_manifest(&manifest)?;
    if manifest_bytes.len() != placeholder_len {
        return Err(SnapshotCodecError::Encode(
            "snapshot manifest changed length while the body was written".into(),
        ));
    }
    let info = manifest_info(&manifest)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(&manifest_bytes);
    let body_start = start + (HEADER_LEN + manifest_bytes.len()) as u64;
    file.seek(SeekFrom::Start(body_start))?;
    let copied = io::copy(
        &mut (&mut *file).take(body_len),
        &mut HashWriter(&mut hasher),
    )?;
    if copied != body_len {
        return Err(SnapshotCodecError::Encode(
            "snapshot body was shorter than written".into(),
        ));
    }
    let checksum = *hasher.finalize().as_bytes();

    file.seek(SeekFrom::Start(start))?;
    write_header(file, manifest_bytes.len(), body_len, &checksum)?;
    file.write_all(&manifest_bytes)?;
    file.seek(SeekFrom::Start(body_start + body_len))?;
    Ok(info)
}

/// Encode a snapshot of `graph` into memory. Holds the finished (compressed)
/// snapshot rather than a copy of every record.
pub fn encode_graph_snapshot(
    graph: &InMemoryGraph,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<(Vec<u8>, SnapshotInfo)> {
    let mut body = Vec::new();
    let counts = write_compressed_body(&mut body, graph, options.compression)?;
    let (body, encryption) = encrypt_snapshot_body(body, options.encryption.as_ref())?;
    encode_envelope(
        body,
        counts.nodes as usize,
        counts.relationships as usize,
        wal_lsn,
        options.compression,
        encryption,
    )
}

#[derive(Clone, Copy)]
struct Counts {
    nodes: u64,
    relationships: u64,
}

fn write_compressed_body<W: Write>(
    out: W,
    graph: &InMemoryGraph,
    compression: Compression,
) -> Result<Counts> {
    match compression {
        Compression::None => {
            let mut out = BufWriter::new(out);
            let counts = write_body(&mut out, graph)?;
            out.flush()?;
            Ok(counts)
        }
        Compression::Gzip { level } => {
            let encoder = GzEncoder::new(out, GzipCompression::new(level.min(9)));
            let mut out = BufWriter::new(encoder);
            let counts = write_body(&mut out, graph)?;
            out.into_inner()
                .map_err(|e| SnapshotCodecError::Io(e.into_error()))?
                .finish()?;
            Ok(counts)
        }
    }
}

/// First-seen dictionary of borrowed strings, numbered as
/// `dictionary_encode_strings` in `columnar` numbers them.
#[derive(Default)]
struct Dictionary<'g> {
    entries: Vec<&'g str>,
    ids: HashMap<&'g str, u32>,
}

impl<'g> Dictionary<'g> {
    fn add(&mut self, value: &'g str) -> Result<()> {
        if !self.ids.contains_key(value) {
            let id = u32::try_from(self.entries.len())
                .map_err(|_| SnapshotCodecError::Encode("string dictionary too large".into()))?;
            self.entries.push(value);
            self.ids.insert(value, id);
        }
        Ok(())
    }

    fn id(&self, value: &str) -> Result<u32> {
        self.ids
            .get(value)
            .copied()
            .ok_or_else(|| SnapshotCodecError::Encode("graph changed while it was written".into()))
    }

    fn write(&self, out: &mut impl Write) -> Result<()> {
        put_len(out, self.entries.len())?;
        for entry in &self.entries {
            put_bytes(out, entry.as_bytes())?;
        }
        Ok(())
    }
}

fn write_body<W: Write>(out: &mut W, graph: &InMemoryGraph) -> Result<Counts> {
    let catalogs = graph.snapshot_catalogs();

    // Prepass: counts and dictionaries, in the order the payload encoder
    // meets them (labels and keys over nodes first, then relationships).
    let mut node_count = 0usize;
    let mut label_count = 0usize;
    let mut property_count = 0usize;
    let mut labels = Dictionary::default();
    let mut keys = Dictionary::default();
    for node in graph.snapshot_nodes() {
        node_count += 1;
        for label in node.labels().strs() {
            label_count += 1;
            labels.add(label)?;
        }
        for key in node.properties().keys() {
            property_count += 1;
            keys.add(key)?;
        }
    }
    let mut rel_count = 0usize;
    let mut types = Dictionary::default();
    for rel in graph.snapshot_relationships() {
        rel_count += 1;
        types.add(rel.rel_type())?;
        for key in rel.properties().keys() {
            property_count += 1;
            keys.add(key)?;
        }
    }

    put_u32(out, BODY_FORMAT_VERSION)?;
    put_u64(out, catalogs.next_node_id)?;
    put_u64(out, catalogs.next_rel_id)?;

    put_len(out, node_count)?;
    for node in graph.snapshot_nodes() {
        put_u64(out, node.id())?;
    }

    labels.write(out)?;
    put_len(out, node_count + 1)?;
    put_u32(out, 0)?;
    let mut offset = 0usize;
    for node in graph.snapshot_nodes() {
        offset += node.labels().len();
        put_u32(
            out,
            u32::try_from(offset).map_err(|_| {
                SnapshotCodecError::Encode("node label offset exceeds u32 range".into())
            })?,
        )?;
    }
    put_len(out, label_count)?;
    for node in graph.snapshot_nodes() {
        for label in node.labels().strs() {
            put_u32(out, labels.id(label)?)?;
        }
    }

    put_len(out, rel_count)?;
    for rel in graph.snapshot_relationships() {
        put_u64(out, rel.id())?;
    }
    put_len(out, rel_count)?;
    for rel in graph.snapshot_relationships() {
        put_u64(out, rel.src())?;
    }
    put_len(out, rel_count)?;
    for rel in graph.snapshot_relationships() {
        put_u64(out, rel.dst())?;
    }
    types.write(out)?;
    put_len(out, rel_count)?;
    for rel in graph.snapshot_relationships() {
        put_u32(out, types.id(rel.rel_type())?)?;
    }

    // Property columns: owner kind, owner index, key id, value.
    put_len(out, property_count)?;
    for node in graph.snapshot_nodes() {
        for _ in 0..node.properties().len() {
            out.write_all(&[0])?;
        }
    }
    for rel in graph.snapshot_relationships() {
        for _ in 0..rel.properties().len() {
            out.write_all(&[1])?;
        }
    }
    put_len(out, property_count)?;
    for (index, node) in graph.snapshot_nodes().enumerate() {
        for _ in 0..node.properties().len() {
            put_u64(out, index as u64)?;
        }
    }
    for (index, rel) in graph.snapshot_relationships().enumerate() {
        for _ in 0..rel.properties().len() {
            put_u64(out, index as u64)?;
        }
    }
    keys.write(out)?;
    put_len(out, property_count)?;
    for node in graph.snapshot_nodes() {
        for key in node.properties().keys() {
            put_u32(out, keys.id(key)?)?;
        }
    }
    for rel in graph.snapshot_relationships() {
        for key in rel.properties().keys() {
            put_u32(out, keys.id(key)?)?;
        }
    }
    put_len(out, property_count)?;
    for node in graph.snapshot_nodes() {
        for (_, value) in node.properties().iter() {
            put_value_ref(out, value)?;
        }
    }
    for rel in graph.snapshot_relationships() {
        for (_, value) in rel.properties().iter() {
            put_value_ref(out, value)?;
        }
    }

    write_trailers(out, &catalogs)?;
    Ok(Counts {
        nodes: node_count as u64,
        relationships: rel_count as u64,
    })
}

fn write_trailers(out: &mut impl Write, catalogs: &SnapshotCatalogs) -> Result<()> {
    let catalog_bytes = encode_index_definitions(&catalogs.indexes)
        .map_err(|e| SnapshotCodecError::Encode(format!("catalog: {e}")))?;
    put_bytes(out, &catalog_bytes)?;
    let constraint_bytes = encode_constraint_definitions(&catalogs.constraints)
        .map_err(|e| SnapshotCodecError::Encode(format!("constraint catalog: {e}")))?;
    put_bytes(out, &constraint_bytes)?;
    let vector_index_bytes = serde_json::to_vec(&catalogs.vector_indexes)
        .map_err(|e| SnapshotCodecError::Encode(format!("vector index trailer: {e}")))?;
    put_bytes(out, &vector_index_bytes)
}

/// A value as `ValueCell::encode_binary` writes it.
fn put_value_ref(out: &mut impl Write, value: ValueRef<'_>) -> Result<()> {
    put_value_ref_at(out, value, 0)
}

/// [`put_value_ref`] for a value nested `depth` levels deep.
fn put_value_ref_at(out: &mut impl Write, value: ValueRef<'_>, depth: usize) -> Result<()> {
    match value {
        ValueRef::Null => out.write_all(&[0])?,
        ValueRef::Bool(value) => out.write_all(&[1, u8::from(value)])?,
        ValueRef::Int(value) => {
            out.write_all(&[2])?;
            out.write_all(&value.to_le_bytes())?;
        }
        ValueRef::Float(value) => {
            out.write_all(&[3])?;
            out.write_all(&value.to_bits().to_le_bytes())?;
        }
        ValueRef::String(value) => {
            out.write_all(&[4])?;
            put_bytes(out, value.as_bytes())?;
        }
        ValueRef::Other(value) => put_value(out, &value.get(), depth)?,
    }
    Ok(())
}

fn put_value(out: &mut impl Write, value: &PropertyValue, depth: usize) -> Result<()> {
    match value {
        PropertyValue::Null
        | PropertyValue::Bool(_)
        | PropertyValue::Int(_)
        | PropertyValue::Float(_)
        | PropertyValue::String(_) => put_value_ref_at(out, ValueRef::from(value), depth)?,
        PropertyValue::Binary(binary) => {
            out.write_all(&[8])?;
            put_len(out, binary.segments().len())?;
            for segment in binary.segments() {
                put_bytes(out, segment)?;
            }
        }
        PropertyValue::List(values) => {
            let depth = nested_encode_depth(depth)?;
            out.write_all(&[5])?;
            put_len(out, values.len())?;
            for value in values {
                put_value(out, value, depth)?;
            }
        }
        PropertyValue::Map(values) => {
            let depth = nested_encode_depth(depth)?;
            out.write_all(&[6])?;
            put_len(out, values.len())?;
            for (key, value) in values {
                put_bytes(out, key.as_bytes())?;
                put_value(out, value, depth)?;
            }
        }
        other => {
            out.write_all(&[7])?;
            let bytes = encode_property_value(other)
                .map_err(|e| SnapshotCodecError::Encode(e.to_string()))?;
            put_bytes(out, &bytes)?;
        }
    }
    Ok(())
}

fn write_header(
    out: &mut impl Write,
    manifest_len: usize,
    body_len: u64,
    checksum: &[u8; 32],
) -> Result<()> {
    let manifest_len = u32::try_from(manifest_len)
        .map_err(|_| SnapshotCodecError::Encode("manifest too large".into()))?;
    out.write_all(MAGIC)?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())?;
    out.write_all(&manifest_len.to_le_bytes())?;
    out.write_all(&body_len.to_le_bytes())?;
    out.write_all(checksum)?;
    Ok(())
}

fn put_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

fn put_u64(out: &mut impl Write, value: u64) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

fn put_len(out: &mut impl Write, len: usize) -> Result<()> {
    let len = u64::try_from(len)
        .map_err(|_| SnapshotCodecError::Encode("length does not fit in u64".into()))?;
    Ok(put_u64(out, len)?)
}

fn put_bytes(out: &mut impl Write, bytes: &[u8]) -> Result<()> {
    put_len(out, bytes.len())?;
    Ok(out.write_all(bytes)?)
}

struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct HashWriter<'a>(&'a mut blake3::Hasher);

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Read the header and manifest of a snapshot without its body. Does not
/// verify the checksum, which covers the body.
pub fn read_snapshot_info<R: Read>(mut reader: R) -> Result<SnapshotInfo> {
    let (manifest, _, _) = read_envelope_head(&mut reader)?;
    manifest_info(&manifest)
}

/// Decode a snapshot from `reader` straight into a graph. Accepts every
/// snapshot [`crate::read_snapshot`] accepts and builds the same graph
/// [`InMemoryGraph::load_snapshot_payload`] would, holding the graph plus
/// the columns described in the module docs instead of the whole file
/// and a decoded copy of every record.
pub fn read_graph_snapshot<R: Read>(
    mut reader: R,
    credentials: Option<&SnapshotCredentials>,
) -> Result<(InMemoryGraph, SnapshotInfo)> {
    let (manifest, manifest_bytes, checksum) = read_envelope_head(&mut reader)?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(
            manifest.format_version,
        ));
    }
    let info = manifest_info(&manifest)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&manifest_bytes);

    let loaded = if matches!(manifest.encryption, EncryptionManifest::None) {
        let mut hashed = HashingReader {
            inner: (&mut reader).take(manifest.body_len),
            hasher: &mut hasher,
            read: 0,
        };
        let loaded = match manifest.compression {
            Compression::None => {
                let mut body = BodyStream::new(BufReader::new(&mut hashed));
                let loaded = read_body(&mut body)?;
                body.expect_end()?;
                loaded
            }
            Compression::Gzip { .. } => {
                let mut body =
                    BodyStream::new(BufReader::new(GzDecoder::new(BufReader::new(&mut hashed))));
                let loaded = read_body(&mut body)?;
                body.expect_end()?;
                loaded
            }
        };
        // The decoder may stop before the end of the compressed body; the
        // checksum covers all of it.
        io::copy(&mut hashed, &mut io::sink())?;
        if hashed.read != manifest.body_len {
            return Err(SnapshotCodecError::Decode(
                "snapshot body is truncated".into(),
            ));
        }
        loaded
    } else {
        let body = read_vec(&mut reader, manifest.body_len, "snapshot body")?;
        hasher.update(&body);
        if hasher.finalize().as_bytes() != &checksum {
            return Err(SnapshotCodecError::ChecksumMismatch);
        }
        let body = decrypt_snapshot_body(&body, &manifest.encryption, credentials)?;
        let loaded = match manifest.compression {
            Compression::None => {
                let mut stream = BodyStream::new(body.as_slice());
                let loaded = read_body(&mut stream)?;
                stream.expect_end()?;
                loaded
            }
            Compression::Gzip { .. } => {
                let mut stream = BodyStream::new(BufReader::new(GzDecoder::new(body.as_slice())));
                let loaded = read_body(&mut stream)?;
                stream.expect_end()?;
                loaded
            }
        };
        let graph = loaded.finish()?;
        return Ok((graph, info));
    };

    if hasher.finalize().as_bytes() != &checksum {
        return Err(SnapshotCodecError::ChecksumMismatch);
    }
    Ok((loaded.finish()?, info))
}

fn read_envelope_head<R: Read>(reader: &mut R) -> Result<(Manifest, Vec<u8>, [u8; 32])> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => {
            SnapshotCodecError::Decode("truncated snapshot header".into())
        }
        _ => SnapshotCodecError::Io(e),
    })?;
    if &header[0..8] != MAGIC {
        return Err(SnapshotCodecError::BadMagic);
    }
    let format_version = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes"));
    if format_version != FORMAT_VERSION {
        return Err(SnapshotCodecError::UnsupportedVersion(format_version));
    }
    let manifest_len = u32::from_le_bytes(header[12..16].try_into().expect("4 bytes"));
    let body_len = u64::from_le_bytes(header[16..24].try_into().expect("8 bytes"));
    let checksum: [u8; 32] = header[24..56].try_into().expect("32 bytes");
    let manifest_bytes = read_vec(reader, u64::from(manifest_len), "snapshot manifest")?;
    let manifest = decode_manifest(&manifest_bytes)?;
    if manifest.body_len != body_len {
        return Err(SnapshotCodecError::Decode(format!(
            "snapshot manifest body length mismatch: manifest says {}, header says {body_len}",
            manifest.body_len
        )));
    }
    Ok((manifest, manifest_bytes, checksum))
}

fn read_vec<R: Read>(reader: &mut R, len: u64, label: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(usize::MAX).min(MAX_PREALLOC));
    let read = reader.take(len).read_to_end(&mut out)?;
    if read as u64 != len {
        return Err(SnapshotCodecError::Decode(format!("{label} is truncated")));
    }
    Ok(out)
}

struct HashingReader<'h, R> {
    inner: R,
    hasher: &'h mut blake3::Hasher,
    read: u64,
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

/// The body read as a stream: the `BodyReader` primitives over a reader.
struct BodyStream<R> {
    inner: R,
}

impl<R: Read> BodyStream<R> {
    fn new(inner: R) -> Self {
        Self { inner }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        self.inner.read_exact(&mut out).map_err(truncated)?;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn len(&mut self) -> Result<usize> {
        usize::try_from(self.u64()?)
            .map_err(|_| SnapshotCodecError::Decode("length overflows usize".into()))
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.u64()?;
        let mut out =
            Vec::with_capacity(usize::try_from(len).unwrap_or(usize::MAX).min(MAX_PREALLOC));
        let read = (&mut self.inner).take(len).read_to_end(&mut out)?;
        if read as u64 != len {
            return Err(SnapshotCodecError::Decode("truncated snapshot body".into()));
        }
        Ok(out)
    }

    fn string(&mut self) -> Result<String> {
        String::from_utf8(self.bytes()?)
            .map_err(|e| SnapshotCodecError::Decode(format!("invalid UTF-8 string: {e}")))
    }

    fn string_vec(&mut self) -> Result<Vec<String>> {
        let len = self.len()?;
        let mut values = Vec::with_capacity(len.min(MAX_PREALLOC));
        for _ in 0..len {
            values.push(self.string()?);
        }
        Ok(values)
    }

    fn u32_vec(&mut self) -> Result<Vec<u32>> {
        let len = self.len()?;
        let mut values = Vec::with_capacity(len.min(MAX_PREALLOC));
        for _ in 0..len {
            values.push(self.u32()?);
        }
        Ok(values)
    }

    fn u64_vec(&mut self) -> Result<Vec<u64>> {
        let len = self.len()?;
        let mut values = Vec::with_capacity(len.min(MAX_PREALLOC));
        for _ in 0..len {
            values.push(self.u64()?);
        }
        Ok(values)
    }

    fn value(&mut self) -> Result<PropertyValue> {
        self.value_at(0)
    }

    /// [`Self::value`] for a value nested `depth` levels deep.
    fn value_at(&mut self, depth: usize) -> Result<PropertyValue> {
        Ok(match self.u8()? {
            0 => PropertyValue::Null,
            1 => PropertyValue::Bool(self.u8()? != 0),
            2 => PropertyValue::Int(i64::from_le_bytes(self.array()?)),
            3 => PropertyValue::Float(f64::from_bits(self.u64()?)),
            4 => PropertyValue::String(self.string()?),
            8 => {
                let len = self.len()?;
                let mut segments = Vec::with_capacity(len.min(MAX_PREALLOC));
                for _ in 0..len {
                    segments.push(self.bytes()?);
                }
                PropertyValue::Binary(lora_store::LoraBinary::from_segments(segments))
            }
            5 => {
                let depth = nested_decode_depth(depth)?;
                let len = self.len()?;
                let mut values = Vec::with_capacity(len.min(MAX_PREALLOC));
                for _ in 0..len {
                    values.push(self.value_at(depth)?);
                }
                PropertyValue::List(values)
            }
            6 => {
                let depth = nested_decode_depth(depth)?;
                let len = self.len()?;
                let mut values = std::collections::BTreeMap::new();
                for _ in 0..len {
                    let key = self.string()?;
                    let value = self.value_at(depth)?;
                    values.insert(key, value);
                }
                PropertyValue::Map(values)
            }
            7 => {
                let bytes = self.bytes()?;
                decode_property_value(&bytes)
                    .map_err(|e| SnapshotCodecError::Decode(e.to_string()))?
            }
            tag => {
                return Err(SnapshotCodecError::Decode(format!(
                    "invalid property value tag {tag}"
                )));
            }
        })
    }

    fn expect_end(&mut self) -> Result<()> {
        let mut rest = Vec::new();
        let trailing = (&mut self.inner).take(1 << 16).read_to_end(&mut rest)?;
        if trailing == 0 {
            Ok(())
        } else {
            Err(SnapshotCodecError::Decode(format!(
                "trailing bytes in snapshot body: {trailing}"
            )))
        }
    }
}

fn truncated(e: io::Error) -> SnapshotCodecError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        SnapshotCodecError::Decode("truncated snapshot body".into())
    } else {
        SnapshotCodecError::Io(e)
    }
}

fn store_error(e: lora_store::SnapshotError) -> SnapshotCodecError {
    SnapshotCodecError::Decode(e.to_string())
}

/// A decoded body waiting for its checksum before the catalogs register
/// (and index) over it.
struct LoadedBody {
    builder: SnapshotGraphBuilder,
    indexes: Vec<IndexDefinition>,
    constraints: Vec<ConstraintDefinition>,
    vector_indexes: Vec<VectorIndexSnapshot>,
}

impl LoadedBody {
    fn finish(self) -> Result<InMemoryGraph> {
        self.builder
            .finish(self.indexes, self.constraints, self.vector_indexes)
            .map_err(store_error)
    }
}

/// Node properties come before relationship properties.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Node,
    Relationship,
}

/// Consecutive properties with one owner.
struct Run {
    owner: Owner,
    index: usize,
    count: usize,
}

/// Which entity each property belongs to, in the order the values come.
enum Owners {
    /// Owners in ascending order, as every encoder writes them: each
    /// entity's property count, 4 bytes an entity.
    Counts {
        nodes: Vec<u32>,
        relationships: Vec<u32>,
    },
    /// Any other order.
    Runs(Vec<Run>),
}

impl Owners {
    fn new(node_count: usize, relationship_count: usize, property_count: usize) -> Self {
        if property_count == 0 {
            return Self::Runs(Vec::new());
        }
        Self::Counts {
            nodes: vec![0; node_count],
            relationships: vec![0; relationship_count],
        }
    }

    /// Record the next property's owner; `last` is the previous one's.
    fn push(&mut self, owner: Owner, index: usize, last: Option<(Owner, usize)>) -> Result<()> {
        if let Self::Counts {
            nodes,
            relationships,
        } = self
        {
            if last.is_none_or(|last| last <= (owner, index)) {
                let count = match owner {
                    Owner::Node => &mut nodes[index],
                    Owner::Relationship => &mut relationships[index],
                };
                *count = count.checked_add(1).ok_or_else(|| {
                    SnapshotCodecError::Decode("entity has too many properties".into())
                })?;
                return Ok(());
            }
            let runs = Self::Counts {
                nodes: std::mem::take(nodes),
                relationships: std::mem::take(relationships),
            }
            .into_runs();
            *self = Self::Runs(runs);
        }
        let Self::Runs(runs) = self else {
            unreachable!("counts were converted to runs");
        };
        match runs.last_mut() {
            Some(run) if run.owner == owner && run.index == index => run.count += 1,
            _ => runs.push(Run {
                owner,
                index,
                count: 1,
            }),
        }
        Ok(())
    }

    fn into_runs(self) -> Vec<Run> {
        match self {
            Self::Runs(runs) => runs,
            Self::Counts {
                nodes,
                relationships,
            } => {
                let runs = |owner, counts: Vec<u32>| {
                    counts
                        .into_iter()
                        .enumerate()
                        .filter(|(_, count)| *count > 0)
                        .map(move |(index, count)| Run {
                            owner,
                            index,
                            count: count as usize,
                        })
                };
                runs(Owner::Node, nodes)
                    .chain(runs(Owner::Relationship, relationships))
                    .collect()
            }
        }
    }
}

/// Each property's key, in value order, held in the narrowest integer the
/// dictionary allows.
struct Keys {
    dictionary: Vec<Arc<str>>,
    ids: KeyIds,
    next: usize,
}

enum KeyIds {
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Keys {
    fn read<R: Read>(body: &mut BodyStream<R>, property_count: usize) -> Result<Self> {
        let dictionary: Vec<Arc<str>> = body
            .string_vec()?
            .into_iter()
            .map(lora_store::intern_owned)
            .collect();
        let len = body.len()?;
        if len != property_count {
            return Err(SnapshotCodecError::Decode(
                "property key id length mismatch".into(),
            ));
        }
        let capacity = len.min(MAX_PREALLOC);
        let mut ids = if dictionary.len() <= 1 << 8 {
            KeyIds::U8(Vec::with_capacity(capacity))
        } else if dictionary.len() <= 1 << 16 {
            KeyIds::U16(Vec::with_capacity(capacity))
        } else {
            KeyIds::U32(Vec::with_capacity(capacity))
        };
        for _ in 0..len {
            let id = body.u32()?;
            if id as usize >= dictionary.len() {
                return Err(SnapshotCodecError::Decode(
                    "invalid property key dictionary id".into(),
                ));
            }
            // The dictionary's length bounds `id` to the variant's width.
            match &mut ids {
                KeyIds::U8(ids) => ids.push(id as u8),
                KeyIds::U16(ids) => ids.push(id as u16),
                KeyIds::U32(ids) => ids.push(id),
            }
        }
        Ok(Self {
            dictionary,
            ids,
            next: 0,
        })
    }

    fn next(&mut self) -> Result<Arc<str>> {
        let id = match &self.ids {
            KeyIds::U8(ids) => ids.get(self.next).map(|id| *id as usize),
            KeyIds::U16(ids) => ids.get(self.next).map(|id| *id as usize),
            KeyIds::U32(ids) => ids.get(self.next).map(|id| *id as usize),
        }
        .ok_or_else(|| SnapshotCodecError::Decode("property key id length mismatch".into()))?;
        self.next += 1;
        Ok(Arc::clone(&self.dictionary[id]))
    }
}

/// The node and relationship columns, read before the properties.
struct Structure {
    node_ids: Vec<u64>,
    label_dictionary: Vec<Name>,
    label_offsets: Vec<u32>,
    label_ids: Vec<u32>,
    rel_ids: Vec<u64>,
    rel_src: Vec<u64>,
    rel_dst: Vec<u64>,
    type_dictionary: Vec<Name>,
    type_ids: Vec<u32>,
}

impl Structure {
    fn node(&self, index: usize, properties: Properties) -> Result<NodeRecord> {
        let start = self.label_offsets[index] as usize;
        let end = self.label_offsets[index + 1] as usize;
        let ids = self
            .label_ids
            .get(start..end)
            .ok_or_else(|| SnapshotCodecError::Decode("invalid node label offset".into()))?;
        let labels = ids
            .iter()
            .map(|id| {
                self.label_dictionary
                    .get(*id as usize)
                    .cloned()
                    .ok_or_else(|| SnapshotCodecError::Decode("invalid label dictionary id".into()))
            })
            .collect::<Result<Labels>>()?;
        Ok(NodeRecord {
            id: self.node_ids[index],
            labels,
            properties,
        })
    }

    fn relationship(&self, index: usize, properties: Properties) -> Result<RelationshipRecord> {
        let rel_type = self
            .type_dictionary
            .get(self.type_ids[index] as usize)
            .cloned()
            .ok_or_else(|| SnapshotCodecError::Decode("invalid relationship type id".into()))?;
        Ok(RelationshipRecord {
            id: self.rel_ids[index],
            src: self.rel_src[index],
            dst: self.rel_dst[index],
            rel_type,
            properties,
        })
    }
}

fn read_body<R: Read>(body: &mut BodyStream<R>) -> Result<LoadedBody> {
    let version = body.u32()?;
    if version != BODY_FORMAT_VERSION
        && version != BODY_FORMAT_VERSION_V4
        && version != BODY_FORMAT_VERSION_V3
        && version != BODY_FORMAT_VERSION_V2
    {
        return Err(SnapshotCodecError::Decode(format!(
            "unsupported snapshot body format version {version}"
        )));
    }
    let next_node_id = body.u64()?;
    let next_rel_id = body.u64()?;
    let mut builder = SnapshotGraphBuilder::new(next_node_id, next_rel_id).map_err(store_error)?;

    let node_ids = body.u64_vec()?;
    let label_dictionary: Vec<Name> = body.string_vec()?.iter().map(Name::from).collect();
    let label_offsets = body.u32_vec()?;
    let label_ids = body.u32_vec()?;
    if label_offsets.len() != node_ids.len() + 1 {
        return Err(SnapshotCodecError::Decode(
            "node label offset length mismatch".into(),
        ));
    }
    let rel_ids = body.u64_vec()?;
    let rel_src = body.u64_vec()?;
    let rel_dst = body.u64_vec()?;
    let type_dictionary: Vec<Name> = body.string_vec()?.iter().map(Name::from).collect();
    let type_ids = body.u32_vec()?;
    if rel_ids.len() != rel_src.len()
        || rel_ids.len() != rel_dst.len()
        || rel_ids.len() != type_ids.len()
    {
        return Err(SnapshotCodecError::Decode(
            "relationship column length mismatch".into(),
        ));
    }
    let structure = Structure {
        node_ids,
        label_dictionary,
        label_offsets,
        label_ids,
        rel_ids,
        rel_src,
        rel_dst,
        type_dictionary,
        type_ids,
    };

    let property_count = body.len()?;
    // Owner kinds, run-length encoded as they arrive.
    let mut kinds: Vec<(Owner, usize)> = Vec::new();
    for _ in 0..property_count {
        let owner = match body.u8()? {
            0 => Owner::Node,
            1 => Owner::Relationship,
            tag => {
                return Err(SnapshotCodecError::Decode(format!(
                    "invalid property owner kind tag {tag}"
                )));
            }
        };
        match kinds.last_mut() {
            Some((last, count)) if *last == owner => *count += 1,
            _ => kinds.push((owner, 1)),
        }
    }
    if body.len()? != property_count {
        return Err(SnapshotCodecError::Decode(
            "property owner index length mismatch".into(),
        ));
    }
    let mut owners = Owners::new(
        structure.node_ids.len(),
        structure.rel_ids.len(),
        property_count,
    );
    let mut last = None;
    for (owner, count) in kinds {
        for _ in 0..count {
            let raw = body.u64()?;
            let index = usize::try_from(raw).map_err(|_| {
                SnapshotCodecError::Decode("property owner does not fit in usize".into())
            })?;
            let bound = match owner {
                Owner::Node => structure.node_ids.len(),
                Owner::Relationship => structure.rel_ids.len(),
            };
            if index >= bound {
                return Err(SnapshotCodecError::Decode(match owner {
                    Owner::Node => "invalid node property owner".into(),
                    Owner::Relationship => "invalid relationship property owner".into(),
                }));
            }
            owners.push(owner, index, last)?;
            last = Some((owner, index));
        }
    }

    let mut keys = Keys::read(body, property_count)?;
    if body.len()? != property_count {
        return Err(SnapshotCodecError::Decode(
            "property value length mismatch".into(),
        ));
    }

    match owners {
        Owners::Counts {
            nodes,
            relationships,
        } => stream_records(
            body,
            &structure,
            &nodes,
            &relationships,
            &mut keys,
            &mut builder,
        )?,
        Owners::Runs(runs) => hold_records(body, &structure, &runs, &mut keys, &mut builder)?,
    }
    drop(keys);
    drop(structure);

    let has_catalog = version >= BODY_FORMAT_VERSION_V3;
    let has_constraints = version >= BODY_FORMAT_VERSION_V4;
    let has_vector_indexes = version >= BODY_FORMAT_VERSION;
    let indexes = if has_catalog {
        decode_index_definitions(&body.bytes()?)
            .map_err(|e| SnapshotCodecError::Decode(format!("catalog: {e}")))?
    } else {
        Vec::new()
    };
    let constraints = if has_constraints {
        decode_constraint_definitions(&body.bytes()?)
            .map_err(|e| SnapshotCodecError::Decode(format!("constraint catalog: {e}")))?
    } else {
        Vec::new()
    };
    let vector_indexes = if has_vector_indexes {
        let bytes = body.bytes()?;
        if bytes.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|e| SnapshotCodecError::Decode(format!("vector index trailer: {e}")))?
        }
    } else {
        Vec::new()
    };

    Ok(LoadedBody {
        builder,
        indexes,
        constraints,
        vector_indexes,
    })
}

fn read_properties<R: Read>(
    body: &mut BodyStream<R>,
    count: usize,
    keys: &mut Keys,
    properties: &mut Properties,
) -> Result<()> {
    for _ in 0..count {
        let key = keys.next()?;
        let value = body.value()?;
        properties.insert(key, value);
    }
    Ok(())
}

/// Build each record as its values arrive and hand it to the builder.
fn stream_records<R: Read>(
    body: &mut BodyStream<R>,
    structure: &Structure,
    node_counts: &[u32],
    relationship_counts: &[u32],
    keys: &mut Keys,
    builder: &mut SnapshotGraphBuilder,
) -> Result<()> {
    for index in 0..structure.node_ids.len() {
        let mut properties = Properties::new();
        let count = node_counts.get(index).copied().unwrap_or(0);
        read_properties(body, count as usize, keys, &mut properties)?;
        builder
            .push_node(structure.node(index, properties)?)
            .map_err(store_error)?;
    }
    for index in 0..structure.rel_ids.len() {
        let mut properties = Properties::new();
        let count = relationship_counts.get(index).copied().unwrap_or(0);
        read_properties(body, count as usize, keys, &mut properties)?;
        builder
            .push_relationship(structure.relationship(index, properties)?)
            .map_err(store_error)?;
    }
    Ok(())
}

/// Fallback for properties out of owner order: hold every record until
/// the values are read.
fn hold_records<R: Read>(
    body: &mut BodyStream<R>,
    structure: &Structure,
    runs: &[Run],
    keys: &mut Keys,
    builder: &mut SnapshotGraphBuilder,
) -> Result<()> {
    let mut nodes = (0..structure.node_ids.len())
        .map(|index| structure.node(index, Properties::new()))
        .collect::<Result<Vec<_>>>()?;
    let mut relationships = (0..structure.rel_ids.len())
        .map(|index| structure.relationship(index, Properties::new()))
        .collect::<Result<Vec<_>>>()?;
    for run in runs {
        let properties = match run.owner {
            Owner::Node => &mut nodes[run.index].properties,
            Owner::Relationship => &mut relationships[run.index].properties,
        };
        read_properties(body, run.count, keys, properties)?;
    }
    for node in nodes {
        builder.push_node(node).map_err(store_error)?;
    }
    for rel in relationships {
        builder.push_relationship(rel).map_err(store_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    use crate::format::MAX_VALUE_DEPTH;

    /// A value `depth` lists deep.
    fn nested(depth: usize) -> PropertyValue {
        let mut value = PropertyValue::Int(1);
        for _ in 0..depth {
            value = PropertyValue::List(vec![value]);
        }
        value
    }

    /// The encoding of [`nested`] (`depth` one-element lists).
    fn crafted(depth: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..depth {
            out.push(5);
            out.extend_from_slice(&1u64.to_le_bytes());
        }
        out.push(0);
        out
    }

    #[test]
    fn value_decode_rejects_nesting_past_the_cap() {
        let at_cap = crafted(MAX_VALUE_DEPTH);
        BodyStream::new(&at_cap[..]).value().unwrap();
        // Deep enough to overflow the stack without the cap.
        let deep = crafted(1_000_000);
        let err = BodyStream::new(&deep[..]).value().unwrap_err();
        assert!(err.to_string().contains("deeper than 128"), "{err}");
    }

    #[test]
    fn value_encode_rejects_nesting_past_the_cap() {
        put_value(&mut Vec::new(), &nested(MAX_VALUE_DEPTH), 0).unwrap();
        let err = put_value(&mut Vec::new(), &nested(MAX_VALUE_DEPTH + 1), 0).unwrap_err();
        assert!(err.to_string().contains("deeper than 128"), "{err}");
    }
}
