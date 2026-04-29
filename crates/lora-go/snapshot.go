package lora

/*
#include <stdlib.h>
#include "lora_ffi.h"
*/
import "C"

import (
	"encoding/base64"
	"encoding/json"
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

// SnapshotCompression selects the database snapshot compression codec.
// Supported formats are "none" and "gzip".
type SnapshotCompression struct {
	Format string `json:"format"`
	Level  uint32 `json:"level,omitempty"`
}

// SnapshotPasswordParams tunes the password KDF used by encrypted snapshots.
// Leave nil for the core interactive defaults.
type SnapshotPasswordParams struct {
	MemoryCostKib uint32 `json:"memoryCostKib,omitempty"`
	TimeCost      uint32 `json:"timeCost,omitempty"`
	Parallelism   uint32 `json:"parallelism,omitempty"`
}

// SnapshotEncryption describes snapshot encryption credentials.
//
// Password encryption is the most portable option across all bindings:
//
//	SnapshotEncryption{Type: "password", Password: "..."}
//
// Raw-key encryption accepts exactly 32 bytes through Key.
type SnapshotEncryption struct {
	Type     string                  `json:"type,omitempty"`
	KeyID    string                  `json:"keyId,omitempty"`
	Password string                  `json:"password,omitempty"`
	Params   *SnapshotPasswordParams `json:"params,omitempty"`
	Key      *[32]byte               `json:"key,omitempty"`
}

// SnapshotOptions controls snapshot save encoding.
type SnapshotOptions struct {
	Compression *SnapshotCompression `json:"compression,omitempty"`
	Encryption  *SnapshotEncryption  `json:"encryption,omitempty"`
}

// SnapshotLoadOptions supplies credentials for encrypted snapshot loads.
// Encryption is accepted so the same encryption block used to save can be
// reused for load.
type SnapshotLoadOptions struct {
	Credentials *SnapshotEncryption `json:"credentials,omitempty"`
	Encryption  *SnapshotEncryption `json:"encryption,omitempty"`
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
	return db.SaveSnapshotWithOptions(path, nil)
}

// SaveSnapshotWithOptions writes the current graph state with explicit
// snapshot codec options, including optional encryption.
func (db *Database) SaveSnapshotWithOptions(path string, options *SnapshotOptions) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}

	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))
	cOptions, cleanup, err := snapshotOptionsCString(options)
	if err != nil {
		return nil, err
	}
	defer cleanup()

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_save_snapshot_with_options(db.handle, cPath, cOptions, &meta, &outError)
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
	return db.SaveSnapshotBytesWithOptions(nil)
}

// SaveSnapshotBytesWithOptions serializes the graph with explicit snapshot
// codec options, including optional encryption.
func (db *Database) SaveSnapshotBytesWithOptions(options *SnapshotOptions) ([]byte, *SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, nil, errClosed()
	}
	cOptions, cleanup, err := snapshotOptionsCString(options)
	if err != nil {
		return nil, nil, err
	}
	defer cleanup()

	var outBytes *C.uint8_t
	var outLen C.size_t
	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_save_snapshot_to_bytes_with_options(db.handle, cOptions, &outBytes, &outLen, &meta, &outError)
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
	return db.SaveSnapshotBase64WithOptions(nil)
}

// SaveSnapshotBase64WithOptions serializes the current graph with explicit
// snapshot options and returns standard base64 text.
func (db *Database) SaveSnapshotBase64WithOptions(options *SnapshotOptions) (string, *SnapshotMeta, error) {
	bytes, meta, err := db.SaveSnapshotBytesWithOptions(options)
	if err != nil {
		return "", nil, err
	}
	return base64.StdEncoding.EncodeToString(bytes), meta, nil
}

// SaveSnapshotTo writes an in-memory snapshot to w.
func (db *Database) SaveSnapshotTo(w io.Writer) (*SnapshotMeta, error) {
	return db.SaveSnapshotToWithOptions(w, nil)
}

// SaveSnapshotToWithOptions writes an in-memory snapshot with explicit
// snapshot options to w.
func (db *Database) SaveSnapshotToWithOptions(w io.Writer, options *SnapshotOptions) (*SnapshotMeta, error) {
	bytes, meta, err := db.SaveSnapshotBytesWithOptions(options)
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
	return db.LoadSnapshotWithOptions(path, nil)
}

// LoadSnapshotWithOptions replaces the current graph state with a snapshot at
// path, supplying credentials for encrypted database snapshots.
func (db *Database) LoadSnapshotWithOptions(path string, options *SnapshotLoadOptions) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}

	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))
	cOptions, cleanup, err := snapshotOptionsCString(options)
	if err != nil {
		return nil, err
	}
	defer cleanup()

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_load_snapshot_with_options(db.handle, cPath, cOptions, &meta, &outError)
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
	return db.LoadSnapshotBytesWithOptions(bytes, nil)
}

// LoadSnapshotBytesWithOptions replaces the graph from in-memory bytes,
// supplying credentials for encrypted database snapshots.
func (db *Database) LoadSnapshotBytesWithOptions(bytes []byte, options *SnapshotLoadOptions) (*SnapshotMeta, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return nil, errClosed()
	}
	if len(bytes) == 0 {
		return nil, &LoraError{Code: CodeInvalidParams, Message: "snapshot bytes are empty"}
	}
	cOptions, cleanup, err := snapshotOptionsCString(options)
	if err != nil {
		return nil, err
	}
	defer cleanup()

	var meta C.LoraSnapshotMeta
	var outError *C.char
	status := C.lora_db_load_snapshot_from_bytes_with_options(
		db.handle,
		(*C.uint8_t)(unsafe.Pointer(&bytes[0])),
		C.size_t(len(bytes)),
		cOptions,
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
	return db.LoadSnapshotBase64WithOptions(encoded, nil)
}

// LoadSnapshotBase64WithOptions decodes standard base64 snapshot text and
// restores it with optional credentials.
func (db *Database) LoadSnapshotBase64WithOptions(encoded string, options *SnapshotLoadOptions) (*SnapshotMeta, error) {
	bytes, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		return nil, err
	}
	return db.LoadSnapshotBytesWithOptions(bytes, options)
}

// LoadSnapshotFrom reads all snapshot bytes from r and restores them.
func (db *Database) LoadSnapshotFrom(r io.Reader) (*SnapshotMeta, error) {
	return db.LoadSnapshotFromWithOptions(r, nil)
}

// LoadSnapshotFromWithOptions reads all snapshot bytes from r and restores
// them with optional credentials.
func (db *Database) LoadSnapshotFromWithOptions(r io.Reader, options *SnapshotLoadOptions) (*SnapshotMeta, error) {
	bytes, err := io.ReadAll(r)
	if err != nil {
		return nil, err
	}
	return db.LoadSnapshotBytesWithOptions(bytes, options)
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

func snapshotOptionsCString(options any) (*C.char, func(), error) {
	if options == nil {
		return nil, func() {}, nil
	}
	bytes, err := json.Marshal(options)
	if err != nil {
		return nil, nil, err
	}
	cString := C.CString(string(bytes))
	return cString, func() { C.free(unsafe.Pointer(cString)) }, nil
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
