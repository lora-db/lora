package lora

/*
#include <stdlib.h>
#include "lora_ffi.h"
*/
import "C"

import (
	"fmt"
	"unsafe"
)

// SnapshotMeta describes a snapshot file. Returned from
// [Database.SaveSnapshot] and [Database.LoadSnapshot]; kept small and
// stable so callers can log or compare it without reflecting on the
// underlying payload.
//
// WalLsn is reserved for the future WAL/checkpoint hybrid: pure
// (non-checkpoint) snapshots always report `WalLsn = nil`.
type SnapshotMeta struct {
	FormatVersion     uint32
	NodeCount         uint64
	RelationshipCount uint64
	WalLsn            *uint64
}

// SaveSnapshot writes the current graph state to `path`. The write is
// atomic: the payload is staged in `<path>.tmp`, fsync'd, and then
// renamed over the target. A crashed save can leave a `.tmp` file
// behind but can never leave a half-written file at `path`.
//
// SaveSnapshot holds the store mutex for the duration of the write, so
// concurrent Execute calls block until the save completes. This matches
// the semantics of the Rust core.
func (db *Database) SaveSnapshot(path string) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}

	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_save_snapshot(db.handle, cPath, &meta, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		return nil, statusToError(int(status), outError)
	}
	return snapshotMetaFromC(&meta), nil
}

// LoadSnapshot replaces the current graph state with the snapshot at
// `path`. Concurrent Execute calls block on the store mutex until the
// load completes.
//
// A missing file is reported as a LoraError; callers who want the
// "optional restore" behaviour used by `lora-server --restore-from`
// should stat the file themselves first.
func (db *Database) LoadSnapshot(path string) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}

	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_load_snapshot(db.handle, cPath, &meta, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		return nil, statusToError(int(status), outError)
	}
	return snapshotMetaFromC(&meta), nil
}

func snapshotMetaFromC(m *C.LoraSnapshotMeta) *SnapshotMeta {
	out := &SnapshotMeta{
		FormatVersion:     uint32(m.format_version),
		NodeCount:         uint64(m.node_count),
		RelationshipCount: uint64(m.relationship_count),
	}
	if m.wal_lsn_set != 0 {
		lsn := uint64(m.wal_lsn)
		out.WalLsn = &lsn
	}
	return out
}

// String renders the metadata in the shape used by the other bindings'
// JSON output — useful for logging without reaching for a JSON encoder.
func (m *SnapshotMeta) String() string {
	lsn := "null"
	if m.WalLsn != nil {
		lsn = fmt.Sprintf("%d", *m.WalLsn)
	}
	return fmt.Sprintf(
		"SnapshotMeta{formatVersion=%d, nodeCount=%d, relationshipCount=%d, walLsn=%s}",
		m.FormatVersion, m.NodeCount, m.RelationshipCount, lsn,
	)
}
