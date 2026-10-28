use std::io::Cursor;

use lora_store::{
    InMemoryGraph, LoraDate, NodeRecord, Properties, PropertyValue, RelationshipRecord,
    SnapshotPayload,
};

use crate::*;

/// Nodes with id gaps, labels shared and not, nested and extension
/// values, relationships with and without properties.
fn payload() -> SnapshotPayload {
    let mut nodes = Vec::new();
    for id in [0u64, 2, 3, 7, 8] {
        let mut properties = Properties::new();
        if id != 3 {
            properties.insert("name".into(), PropertyValue::String(format!("n{id}")));
            properties.insert("rank".into(), PropertyValue::Int(id as i64));
        }
        if id == 7 {
            let mut map = std::collections::BTreeMap::new();
            map.insert("a".to_string(), PropertyValue::Float(1.5));
            map.insert(
                "b".to_string(),
                PropertyValue::List(vec![PropertyValue::Null, PropertyValue::Bool(false)]),
            );
            properties.insert("nested".into(), PropertyValue::Map(map));
            properties.insert(
                "born".into(),
                PropertyValue::Date(LoraDate::new(2001, 2, 3).unwrap()),
            );
            properties.insert(
                "blob".into(),
                PropertyValue::Binary(lora_store::LoraBinary::from_segments(vec![
                    vec![1, 2],
                    vec![3],
                ])),
            );
        }
        let labels: Vec<String> = match id {
            0 => vec!["User".into(), "Admin".into()],
            8 => vec![],
            _ => vec!["User".into()],
        };
        nodes.push(NodeRecord {
            id,
            labels: labels.into(),
            properties,
        });
    }
    let mut relationships = Vec::new();
    for (id, src, dst) in [(1u64, 0u64, 2u64), (4, 2, 7), (5, 7, 7)] {
        let mut properties = Properties::new();
        if id != 4 {
            properties.insert("since".into(), PropertyValue::Int(2000 + id as i64));
        }
        relationships.push(RelationshipRecord {
            id,
            src,
            dst,
            rel_type: if id == 5 { "SELF" } else { "KNOWS" }.into(),
            properties,
        });
    }
    SnapshotPayload {
        next_node_id: 10,
        next_rel_id: 6,
        nodes,
        relationships,
        indexes: Vec::new(),
        constraints: Vec::new(),
        vector_indexes: Vec::new(),
    }
}

fn graph() -> InMemoryGraph {
    let mut graph = InMemoryGraph::new();
    graph.load_snapshot_payload(payload()).unwrap();
    graph
}

fn options(compression: Compression) -> SnapshotOptions {
    SnapshotOptions {
        compression,
        encryption: None,
    }
}

fn write_to_vec(graph: &InMemoryGraph, wal_lsn: Option<u64>, options: &SnapshotOptions) -> Vec<u8> {
    let mut file = Cursor::new(Vec::new());
    write_graph_snapshot(&mut file, graph, wal_lsn, options).unwrap();
    file.into_inner()
}

#[test]
fn streamed_and_in_memory_writers_agree_and_round_trip() {
    let graph = graph();
    for compression in [Compression::None, Compression::Gzip { level: 1 }] {
        let options = options(compression);
        let streamed = write_to_vec(&graph, Some(42), &options);
        let (in_memory, _) = encode_graph_snapshot(&graph, Some(42), &options).unwrap();
        assert_eq!(streamed, in_memory, "{compression:?}");
        assert_eq!(
            u32::from_le_bytes(streamed[8..12].try_into().unwrap()),
            crate::format::FORMAT_VERSION_CHUNKED
        );
        let (loaded, info) = read_graph_snapshot(streamed.as_slice(), None).unwrap();
        assert_eq!(loaded.snapshot_payload(), graph.snapshot_payload());
        assert_eq!(info.format_version, crate::format::FORMAT_VERSION_CHUNKED);
        let (decoded, _) = decode_snapshot(&streamed, None).unwrap();
        assert_eq!(decoded, graph.snapshot_payload());
    }
}

#[test]
fn column_major_snapshots_still_load() {
    let graph = graph();
    for compression in [Compression::None, Compression::Gzip { level: 1 }] {
        let options = options(compression);
        let legacy =
            encode_snapshot_with_options(&graph.snapshot_payload(), Some(42), &options).unwrap();
        assert_eq!(
            u32::from_le_bytes(legacy[8..12].try_into().unwrap()),
            crate::format::FORMAT_VERSION
        );
        let (loaded, info) = read_graph_snapshot(legacy.as_slice(), None).unwrap();
        assert_eq!(loaded.snapshot_payload(), graph.snapshot_payload());
        assert_eq!(info.wal_lsn, Some(42));
    }
}

#[test]
fn streamed_snapshot_lands_at_the_writer_position() {
    let graph = graph();
    let options = options(Compression::Gzip { level: 1 });
    let mut file = Cursor::new(b"prefix".to_vec());
    file.set_position(6);
    write_graph_snapshot(&mut file, &graph, None, &options).unwrap();
    assert_eq!(file.position() as usize, file.get_ref().len());
    let bytes = file.into_inner();
    assert_eq!(&bytes[..6], b"prefix");
    let (decoded, _) = decode_snapshot(&bytes[6..], None).unwrap();
    assert_eq!(decoded, graph.snapshot_payload());
}

#[test]
fn streamed_read_builds_the_graph_the_payload_decoder_builds() {
    let graph = graph();
    for compression in [Compression::None, Compression::Gzip { level: 6 }] {
        let bytes = write_to_vec(&graph, Some(9), &options(compression));
        let (loaded, info) = read_graph_snapshot(bytes.as_slice(), None).unwrap();
        assert_eq!(loaded.snapshot_payload(), payload());
        assert_eq!(info.wal_lsn, Some(9));
        assert_eq!(info.node_count, 5);
        assert_eq!(info.relationship_count, 3);
        assert_eq!(read_snapshot_info(bytes.as_slice()).unwrap(), info);
    }
}

#[test]
fn streamed_read_and_write_handle_encryption() {
    let graph = graph();
    let key = EncryptionKey::new("k", [3; 32]);
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 1 },
        encryption: Some(key.clone().into()),
    };
    let bytes = write_to_vec(&graph, Some(5), &options);
    assert!(matches!(
        read_graph_snapshot(bytes.as_slice(), None),
        Err(SnapshotCodecError::MissingEncryptionKey(_))
    ));
    let credentials = SnapshotEncryption::from(key);
    let (loaded, info) = read_graph_snapshot(bytes.as_slice(), Some(&credentials)).unwrap();
    assert_eq!(loaded.snapshot_payload(), payload());
    assert!(info.encrypted);
    let (decoded, _) = decode_snapshot(&bytes, Some(&credentials)).unwrap();
    assert_eq!(decoded, payload());
}

#[test]
fn streamed_read_rejects_a_tampered_or_truncated_body() {
    let graph = graph();
    for compression in [Compression::None, Compression::Gzip { level: 1 }] {
        let bytes = write_to_vec(&graph, None, &options(compression));

        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x55;
        assert!(read_graph_snapshot(tampered.as_slice(), None).is_err());

        let truncated = &bytes[..bytes.len() - 3];
        assert!(read_graph_snapshot(truncated, None).is_err());
    }
}

#[test]
fn streamed_read_accepts_properties_out_of_owner_order() {
    // Body written by hand: node 1's property before node 0's.
    let mut body = Vec::new();
    let put_u64 = |out: &mut Vec<u8>, v: u64| out.extend_from_slice(&v.to_le_bytes());
    let put_u32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    let put_str = |out: &mut Vec<u8>, s: &str| {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    put_u32(&mut body, 5);
    put_u64(&mut body, 2); // next node id
    put_u64(&mut body, 0); // next rel id
    put_u64(&mut body, 2); // node ids
    put_u64(&mut body, 0);
    put_u64(&mut body, 1);
    put_u64(&mut body, 0); // label dictionary
    put_u64(&mut body, 3); // label offsets
    put_u32(&mut body, 0);
    put_u32(&mut body, 0);
    put_u32(&mut body, 0);
    put_u64(&mut body, 0); // label ids
    for _ in 0..3 {
        put_u64(&mut body, 0); // rel ids, src, dst
    }
    put_u64(&mut body, 0); // type dictionary
    put_u64(&mut body, 0); // type ids
    put_u64(&mut body, 2); // property owner kinds
    body.extend_from_slice(&[0, 0]);
    put_u64(&mut body, 2); // owner indexes
    put_u64(&mut body, 1);
    put_u64(&mut body, 0);
    put_u64(&mut body, 1); // key dictionary
    put_str(&mut body, "k");
    put_u64(&mut body, 2); // key ids
    put_u32(&mut body, 0);
    put_u32(&mut body, 0);
    put_u64(&mut body, 2); // values
    body.push(2);
    body.extend_from_slice(&11i64.to_le_bytes());
    body.push(2);
    body.extend_from_slice(&10i64.to_le_bytes());
    for trailer in [
        lora_store::codec::encode_index_definitions(&[]).unwrap(),
        lora_store::codec::encode_constraint_definitions(&[]).unwrap(),
        b"[]".to_vec(),
    ] {
        put_u64(&mut body, trailer.len() as u64);
        body.extend_from_slice(&trailer);
    }

    let bytes = crate::envelope::encode_envelope(
        crate::format::FORMAT_VERSION,
        body,
        2,
        0,
        None,
        Compression::None,
        crate::envelope::EncryptionManifest::None,
    )
    .unwrap()
    .0;
    let (decoded, _) = decode_snapshot(&bytes, None).unwrap();
    let (loaded, _) = read_graph_snapshot(bytes.as_slice(), None).unwrap();
    assert_eq!(loaded.snapshot_payload(), decoded);
    let rank = |id: usize| decoded.nodes[id].properties.get("k").cloned();
    assert_eq!(rank(0), Some(PropertyValue::Int(10)));
    assert_eq!(rank(1), Some(PropertyValue::Int(11)));
}

/// A graph spread over many frames, with ids freed by deletes, several
/// labels, a key whose few values the graph stores by number, and values
/// of every shape.
fn framed_graph() -> InMemoryGraph {
    use lora_store::{intern, GraphStorageMut};
    let mut graph = InMemoryGraph::new();
    let mut ids = Vec::new();
    for i in 0..40_000u64 {
        let labels = match i % 4 {
            0 => vec!["Person".to_string()],
            1 => vec!["Person".to_string(), "Admin".to_string()],
            2 => vec!["Place".to_string()],
            _ => Vec::new(),
        };
        let mut properties = Properties::new();
        properties.insert(intern("name"), PropertyValue::String(format!("n{i}")));
        properties.insert(
            intern("source"),
            PropertyValue::String(["musicbrainz", "discogs", "manual"][(i % 3) as usize].into()),
        );
        properties.insert(intern("rank"), PropertyValue::Int(i as i64 - 20_000));
        if i % 5 == 0 {
            properties.insert(intern("score"), PropertyValue::Float(i as f64 / 7.0));
        }
        if i % 9 == 0 {
            properties.insert(
                intern("tags"),
                PropertyValue::List(vec![
                    PropertyValue::Int(1),
                    PropertyValue::String("t".into()),
                ]),
            );
        }
        if i % 13 == 0 {
            let mut map = std::collections::BTreeMap::new();
            map.insert("k".to_string(), PropertyValue::Bool(i % 2 == 0));
            properties.insert(intern("meta"), PropertyValue::Map(map));
        }
        ids.push(graph.create_node(labels, properties).id);
    }
    for (i, pair) in ids.windows(2).enumerate() {
        let mut properties = Properties::new();
        if i % 2 == 0 {
            properties.insert(intern("weight"), PropertyValue::Int(i as i64 % 17));
        }
        let rel_type = if i % 3 == 0 { "NEXT" } else { "KNOWS" };
        graph.create_relationship(pair[0], pair[1], rel_type, properties);
    }
    // Holes in both id spaces, some spanning a frame boundary.
    for id in (8_000..8_400).chain(20_000..20_010) {
        graph.detach_delete_node(id);
    }
    graph
}

#[test]
fn a_graph_over_many_frames_round_trips_with_its_statistics_and_dictionaries() {
    let graph = framed_graph();
    for compression in [
        Compression::None,
        Compression::Gzip { level: 1 },
        Compression::Zstd { level: 3 },
    ] {
        let bytes = write_to_vec(&graph, Some(7), &options(compression));
        let (loaded, info) = read_graph_snapshot(bytes.as_slice(), None).unwrap();
        assert_eq!(info.node_count, graph.snapshot_payload().nodes.len());
        assert_eq!(loaded.snapshot_payload(), graph.snapshot_payload());
        assert_eq!(loaded.graph_stats(), graph.graph_stats());
        // The values the writer stored by number are stored by number again.
        let values = loaded.snapshot_names().values;
        assert_eq!(values, graph.snapshot_names().values);
        assert!(!values.strings.is_empty());
        // And a write after the restore allocates past the old ids.
        let mut loaded = loaded;
        use lora_store::GraphStorageMut;
        let next = loaded.create_node(Vec::new(), Properties::new()).id;
        assert_eq!(next, 40_000);
    }
}

#[test]
fn a_corrupt_frame_is_refused() {
    let graph = framed_graph();
    let bytes = write_to_vec(&graph, None, &options(Compression::Gzip { level: 1 }));
    // Flip a byte in the middle of the body: the checksum catches it even
    // when the frame still inflates.
    let mut tampered = bytes.clone();
    let at = tampered.len() / 2;
    tampered[at] ^= 0x55;
    assert!(read_graph_snapshot(tampered.as_slice(), None).is_err());
    assert!(read_graph_snapshot(&bytes[..bytes.len() - 1], None).is_err());
}
