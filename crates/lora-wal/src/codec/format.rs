//! On-disk constants shared between encode and decode paths.

pub(super) const PAYLOAD_MAGIC: &[u8; 4] = b"LW1\0";

pub(super) const TAG_CREATE_NODE: u8 = 1;
pub(super) const TAG_CREATE_RELATIONSHIP: u8 = 2;
pub(super) const TAG_SET_NODE_PROPERTY: u8 = 3;
pub(super) const TAG_REMOVE_NODE_PROPERTY: u8 = 4;
pub(super) const TAG_ADD_NODE_LABEL: u8 = 5;
pub(super) const TAG_REMOVE_NODE_LABEL: u8 = 6;
pub(super) const TAG_SET_RELATIONSHIP_PROPERTY: u8 = 7;
pub(super) const TAG_REMOVE_RELATIONSHIP_PROPERTY: u8 = 8;
pub(super) const TAG_DELETE_RELATIONSHIP: u8 = 9;
pub(super) const TAG_DELETE_NODE: u8 = 10;
pub(super) const TAG_DETACH_DELETE_NODE: u8 = 11;
pub(super) const TAG_CLEAR: u8 = 12;

pub(super) const VALUE_NULL: u8 = 0;
pub(super) const VALUE_BOOL: u8 = 1;
pub(super) const VALUE_INT: u8 = 2;
pub(super) const VALUE_FLOAT: u8 = 3;
pub(super) const VALUE_STRING: u8 = 4;
pub(super) const VALUE_LIST: u8 = 5;
pub(super) const VALUE_MAP: u8 = 6;
pub(super) const VALUE_DATE: u8 = 7;
pub(super) const VALUE_TIME: u8 = 8;
pub(super) const VALUE_LOCAL_TIME: u8 = 9;
pub(super) const VALUE_DATE_TIME: u8 = 10;
pub(super) const VALUE_LOCAL_DATE_TIME: u8 = 11;
pub(super) const VALUE_DURATION: u8 = 12;
pub(super) const VALUE_POINT: u8 = 13;
pub(super) const VALUE_VECTOR: u8 = 14;
pub(super) const VALUE_BINARY: u8 = 15;

pub(super) const VECTOR_FLOAT64: u8 = 1;
pub(super) const VECTOR_FLOAT32: u8 = 2;
pub(super) const VECTOR_INTEGER64: u8 = 3;
pub(super) const VECTOR_INTEGER32: u8 = 4;
pub(super) const VECTOR_INTEGER16: u8 = 5;
pub(super) const VECTOR_INTEGER8: u8 = 6;
