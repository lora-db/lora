//! [`WalMirror`] — optional side-effect after the WAL has flushed.

use std::path::Path;

use crate::errors::WalError;

/// Optional side-effect after the WAL has successfully flushed.
///
/// The core WAL stays directory/segment based for append performance. Higher
/// layers can install a mirror to copy that durable directory into another
/// representation, such as the portable `.loradb` archive file used by named
/// databases.
pub trait WalMirror: Send + Sync {
    fn persist(&self, wal_dir: &Path) -> Result<(), WalError>;

    fn persist_force(&self, wal_dir: &Path) -> Result<(), WalError> {
        self.persist(wal_dir)
    }
}
