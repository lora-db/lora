package lora

/*
#include <stdlib.h>
#include "lora_ffi.h"
*/
import "C"

import (
	"encoding/base64"
	"fmt"
	"io"
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
// SaveSnapshot holds the store read lock for the duration of the write,
// so concurrent writes block until the save completes. This matches the
// semantics of the Rust core.
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

// SaveSnapshotBytes serializes the current graph into an in-memory snapshot.
func (db *Database) SaveSnapshotBytes() ([]byte, *SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, nil, errClosed()
	}

	var outBytes *C.uint8_t
	var outLen C.size_t
	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_save_snapshot_to_bytes(db.handle, &outBytes, &outLen, &meta, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		return nil, nil, statusToError(int(status), outError)
	}
	defer C.lora_bytes_free(outBytes, outLen)

	bytes := C.GoBytes(unsafe.Pointer(outBytes), C.int(outLen))
	return bytes, snapshotMetaFromC(&meta), nil
}

// SaveSnapshotBase64 serializes the current graph and returns the snapshot
// encoded with standard base64.
func (db *Database) SaveSnapshotBase64() (string, *SnapshotMeta, error) {
	bytes, meta, err := db.SaveSnapshotBytes()
	if err != nil {
		return "", nil, err
	}
	return base64.StdEncoding.EncodeToString(bytes), meta, nil
}

// SaveSnapshotTo writes an in-memory snapshot to w.
func (db *Database) SaveSnapshotTo(w io.Writer) (*SnapshotMeta, error) {
	bytes, meta, err := db.SaveSnapshotBytes()
	if err != nil {
		return nil, err
	}
	if _, err := w.Write(bytes); err != nil {
		return nil, err
	}
	return meta, nil
}

// LoadSnapshot replaces the current graph state with the snapshot at
// `path`. Concurrent Execute calls block on the store write lock until
// the load completes.
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

// LoadSnapshotBytes replaces the current graph state from in-memory
// snapshot bytes.
func (db *Database) LoadSnapshotBytes(bytes []byte) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}
	if len(bytes) == 0 {
		return nil, &LoraError{Code: CodeInvalidParams, Message: "snapshot bytes are empty"}
	}

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_load_snapshot_from_bytes(
		db.handle,
		(*C.uint8_t)(unsafe.Pointer(&bytes[0])),
		C.size_t(len(bytes)),
		&meta,
		&outError,
	)
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

// LoadSnapshotBase64 decodes standard base64 snapshot text and restores it.
func (db *Database) LoadSnapshotBase64(encoded string) (*SnapshotMeta, error) {
	bytes, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, err
	}
	return db.LoadSnapshotBytes(bytes)
}

// LoadSnapshotFrom reads all snapshot bytes from r and restores them.
func (db *Database) LoadSnapshotFrom(r io.Reader) (*SnapshotMeta, error) {
	bytes, err := io.ReadAll(r)
	if err != nil {
		return nil, err
	}
	return db.LoadSnapshotBytes(bytes)
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
