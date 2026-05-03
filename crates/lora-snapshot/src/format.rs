//! On-disk format constants for the columnar snapshot.

pub(crate) const MAGIC: &[u8; 8] = b"LORACOL1";
pub const DATABASE_SNAPSHOT_MAGIC: &[u8; 8] = MAGIC;
pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 32;
pub(crate) const BODY_FORMAT_VERSION: u32 = 2;
