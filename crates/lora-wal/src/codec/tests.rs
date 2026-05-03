use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, MutationEvent, Properties, PropertyValue, VectorValues,
};

use super::decode::{decode_event, decode_events};
use super::encode::{encode_event, encode_events, encoded_event_len, encoded_events_len};
use crate::errors::WalError;

fn sample_event() -> MutationEvent {
    let mut props = Properties::new();
    props.insert("name".into(), PropertyValue::String("alice".into()));
    props.insert("age".into(), PropertyValue::Int(42));
    props.insert(
        "blob".into(),
        PropertyValue::Binary(LoraBinary::from_segments(vec![
            vec![0, 1, 2],
            vec![3, 4, 255],
        ])),
    );
    props.insert(
        "nested".into(),
        PropertyValue::List(vec![PropertyValue::Bool(true), PropertyValue::Null]),
    );
    MutationEvent::CreateNode {
        id: 7,
        labels: vec!["Person".into(), "Admin".into()],
        properties: props,
    }
}

fn all_extension_values_event() -> MutationEvent {
    let mut props = Properties::new();
    props.insert(
        "date".into(),
        PropertyValue::Date(LoraDate {
            year: 2026,
            month: 4,
            day: 27,
        }),
    );
    props.insert(
        "time".into(),
        PropertyValue::Time(LoraTime {
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 789,
            offset_seconds: 3600,
        }),
    );
    props.insert(
        "localtime".into(),
        PropertyValue::LocalTime(LoraLocalTime {
            hour: 1,
            minute: 2,
            second: 3,
            nanosecond: 4,
        }),
    );
    props.insert(
        "datetime".into(),
        PropertyValue::DateTime(LoraDateTime {
            year: 2026,
            month: 4,
            day: 27,
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 789,
            offset_seconds: -1800,
        }),
    );
    props.insert(
        "localdatetime".into(),
        PropertyValue::LocalDateTime(LoraLocalDateTime {
            year: 2026,
            month: 4,
            day: 27,
            hour: 12,
            minute: 34,
            second: 56,
            nanosecond: 789,
        }),
    );
    props.insert(
        "duration".into(),
        PropertyValue::Duration(LoraDuration {
            months: 14,
            days: 3,
            seconds: 4,
            nanoseconds: 5,
        }),
    );
    props.insert(
        "point".into(),
        PropertyValue::Point(LoraPoint {
            x: 4.9,
            y: 52.37,
            z: Some(7.0),
            srid: 4979,
        }),
    );
    props.insert(
        "vector_f64".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Float64(vec![1.5, 2.5]),
        }),
    );
    props.insert(
        "vector_f32".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Float32(vec![1.5, 2.5]),
        }),
    );
    props.insert(
        "vector_i64".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Integer64(vec![1, -2]),
        }),
    );
    props.insert(
        "vector_i32".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Integer32(vec![1, -2]),
        }),
    );
    props.insert(
        "vector_i16".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Integer16(vec![1, -2]),
        }),
    );
    props.insert(
        "vector_i8".into(),
        PropertyValue::Vector(LoraVector {
            dimension: 2,
            values: VectorValues::Integer8(vec![1, -2]),
        }),
    );
    MutationEvent::SetNodeProperty {
        node_id: 7,
        key: "extensions".into(),
        value: PropertyValue::Map(props),
    }
}

#[test]
fn event_round_trip() {
    let event = sample_event();
    let encoded = encode_event(&event).unwrap();
    assert_eq!(encoded_event_len(&event).unwrap(), encoded.len());
    assert_eq!(decode_event(&encoded).unwrap(), event);
}

#[test]
fn event_batch_round_trip() {
    let events = vec![sample_event(), MutationEvent::Clear];
    let encoded = encode_events(&events).unwrap();
    assert_eq!(encoded_events_len(&events).unwrap(), encoded.len());
    assert_eq!(decode_events(&encoded).unwrap(), events);
}

#[test]
fn non_compact_payload_is_rejected() {
    assert!(matches!(
        decode_event(b"not-lora-wal"),
        Err(WalError::Decode(_))
    ));
}

#[test]
fn add_label_round_trip() {
    let event = MutationEvent::AddNodeLabel {
        node_id: 99,
        label: "User".into(),
    };
    let encoded = encode_event(&event).unwrap();
    assert_eq!(decode_event(&encoded).unwrap(), event);
}

#[test]
fn all_extension_values_round_trip() {
    let event = all_extension_values_event();
    let encoded = encode_event(&event).unwrap();
    assert_eq!(encoded_event_len(&event).unwrap(), encoded.len());
    assert_eq!(decode_event(&encoded).unwrap(), event);
}
