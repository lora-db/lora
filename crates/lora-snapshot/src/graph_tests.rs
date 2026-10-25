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
fn streamed_snapshot_matches_the_payload_encoder_byte_for_byte() {
    let graph = graph();
    for compression in [Compression::None, Compression::Gzip { level: 1 }] {
        let options = options(compression);
        let streamed = write_to_vec(&graph, Some(42), &options);
        let buffered =
            encode_snapshot_with_options(&graph.snapshot_payload(), Some(42), &options).unwrap();
        assert_eq!(streamed, buffered, "{compression:?}");
        let (in_memory, _) = encode_graph_snapshot(&graph, Some(42), &options).unwrap();
        assert_eq!(in_memory, buffered, "{compression:?}");
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
