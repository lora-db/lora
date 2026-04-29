use std::collections::BTreeMap;

use lora_store::{NodeRecord, PropertyValue, RelationshipRecord, SnapshotPayload};

use crate::*;

fn payload() -> SnapshotPayload {
    let mut alice_props = BTreeMap::new();
    alice_props.insert("name".into(), PropertyValue::String("alice".into()));
    alice_props.insert("age".into(), PropertyValue::Int(42));
    alice_props.insert(
        "avatar".into(),
        PropertyValue::Binary(lora_store::LoraBinary::from_segments(vec![
            vec![0, 1, 2],
            vec![3, 4, 255],
        ])),
    );
    alice_props.insert(
        "tags".into(),
        PropertyValue::List(vec![
            PropertyValue::String("admin".into()),
            PropertyValue::Bool(true),
        ]),
    );

    let mut rel_props = BTreeMap::new();
    rel_props.insert("since".into(), PropertyValue::Int(2024));

    SnapshotPayload {
        next_node_id: 2,
        next_rel_id: 1,
        nodes: vec![
            NodeRecord {
                id: 0,
                labels: vec!["User".into(), "Admin".into()],
                properties: alice_props,
            },
            NodeRecord {
                id: 1,
                labels: vec!["User".into()],
                properties: BTreeMap::new(),
            },
        ],
        relationships: vec![RelationshipRecord {
            id: 0,
            src: 0,
            dst: 1,
            rel_type: "KNOWS".into(),
            properties: rel_props,
        }],
    }
}

#[test]
fn roundtrip_compressed_snapshot() {
    let payload = payload();
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 1 },
        encryption: None,
    };
    let bytes = encode_snapshot_with_options(&payload, Some(7), &options).unwrap();
    let (decoded, info) = decode_snapshot(&bytes, None).unwrap();
    assert_eq!(decoded, payload);
    assert_eq!(info.wal_lsn, Some(7));
    assert_eq!(info.node_count, 2);
    assert_eq!(info.relationship_count, 1);
    assert_eq!(info.compression, Compression::Gzip { level: 1 });
    assert!(!info.encrypted);
}

#[test]
fn roundtrip_encrypted_snapshot() {
    let payload = payload();
    let key = EncryptionKey::new("test-key", [9; 32]);
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 1 },
        encryption: Some(key.clone().into()),
    };
    let bytes = encode_snapshot_with_options(&payload, Some(11), &options).unwrap();
    assert!(matches!(
        decode_snapshot(&bytes, None),
        Err(SnapshotCodecError::MissingEncryptionKey(_))
    ));
    let credentials = SnapshotEncryption::from(key);
    let (decoded, info) = decode_snapshot(&bytes, Some(&credentials)).unwrap();
    assert_eq!(decoded, payload);
    assert_eq!(info.wal_lsn, Some(11));
    assert_eq!(info.compression, Compression::Gzip { level: 1 });
    assert_eq!(info.key_id.as_deref(), Some("test-key"));
    assert!(info.encrypted);
}

#[test]
fn roundtrip_password_encrypted_snapshot() {
    let payload = payload();
    let password = SnapshotPassword::with_params(
        "local-password",
        "correct horse battery staple",
        PasswordKdfParams {
            memory_cost_kib: 512,
            time_cost: 1,
            parallelism: 1,
        },
    );
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 6 },
        encryption: Some(password.clone().into()),
    };
    let bytes = encode_snapshot_with_options(&payload, Some(17), &options).unwrap();
    assert!(matches!(
        decode_snapshot(&bytes, None),
        Err(SnapshotCodecError::MissingPassword(_))
    ));
    let credentials = SnapshotEncryption::from(password);
    let (decoded, info) = decode_snapshot(&bytes, Some(&credentials)).unwrap();
    assert_eq!(decoded, payload);
    assert_eq!(info.wal_lsn, Some(17));
    assert_eq!(info.compression, Compression::Gzip { level: 6 });
    assert_eq!(info.key_id.as_deref(), Some("local-password"));
    assert!(info.encrypted);
}

#[test]
fn checksum_rejects_tampering() {
    let payload = payload();
    let mut bytes = encode_snapshot(&payload, None).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xaa;
    assert!(matches!(
        decode_snapshot(&bytes, None),
        Err(SnapshotCodecError::ChecksumMismatch)
    ));
}

#[test]
fn info_path_does_not_decode_body() {
    let payload = payload();
    let bytes = encode_snapshot(&payload, Some(23)).unwrap();
    let info = snapshot_info(&bytes).unwrap();
    assert_eq!(info.wal_lsn, Some(23));
    assert_eq!(info.node_count, 2);
    assert_eq!(info.relationship_count, 1);
    assert_eq!(info.compression, SnapshotOptions::default().compression);
}

#[test]
fn zero_copy_view_reads_uncompressed_columns() {
    let payload = payload();
    let options = SnapshotOptions {
        compression: Compression::None,
        encryption: None,
    };
    let bytes = encode_snapshot_with_options(&payload, Some(31), &options).unwrap();
    let view = open_snapshot_view(&bytes).unwrap();
    assert_eq!(view.info().wal_lsn, Some(31));
    assert_eq!(view.node_ids().len(), 2);
    assert_eq!(view.node_ids().get(0), Some(0));
    assert_eq!(view.node_ids().get(1), Some(1));
    let labels = view.labels_for_node_index(0).unwrap().collect::<Vec<_>>();
    assert_eq!(labels, vec!["User", "Admin"]);
    assert_eq!(view.relationship_ids().get(0), Some(0));
    assert_eq!(view.relationship_sources().get(0), Some(0));
    assert_eq!(view.relationship_targets().get(0), Some(1));
    assert_eq!(
        view.relationship_type(view.relationship_type_ids().get(0).unwrap()),
        Some("KNOWS")
    );
}

#[test]
fn zero_copy_view_rejects_transformed_bodies() {
    let payload = payload();
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 1 },
        encryption: None,
    };
    let bytes = encode_snapshot_with_options(&payload, Some(23), &options).unwrap();
    assert!(matches!(
        open_snapshot_view(&bytes),
        Err(SnapshotCodecError::Decode(_))
    ));
}

#[test]
fn large_columnar_roundtrip() {
    let mut nodes = Vec::new();
    let mut relationships = Vec::new();
    for id in 0..1_000u64 {
        let mut properties = BTreeMap::new();
        properties.insert("name".into(), PropertyValue::String(format!("user-{id}")));
        properties.insert("rank".into(), PropertyValue::Int(id as i64));
        properties.insert("active".into(), PropertyValue::Bool(id % 2 == 0));
        nodes.push(NodeRecord {
            id,
            labels: vec!["User".into(), format!("Bucket{}", id % 8)],
            properties,
        });
    }
    for id in 0..999u64 {
        relationships.push(RelationshipRecord {
            id,
            src: id,
            dst: id + 1,
            rel_type: if id % 2 == 0 { "FOLLOWS" } else { "KNOWS" }.into(),
            properties: BTreeMap::new(),
        });
    }
    let payload = SnapshotPayload {
        next_node_id: 1_000,
        next_rel_id: 999,
        nodes,
        relationships,
    };
    let options = SnapshotOptions {
        compression: Compression::Gzip { level: 1 },
        encryption: None,
    };
    let bytes = encode_snapshot_with_options(&payload, Some(99), &options).unwrap();
    let info = snapshot_info(&bytes).unwrap();
    assert_eq!(info.node_count, 1_000);
    assert_eq!(info.relationship_count, 999);
    assert_eq!(info.compression, Compression::Gzip { level: 1 });
    let (decoded, _) = decode_snapshot(&bytes, None).unwrap();
    assert_eq!(decoded, payload);
}
