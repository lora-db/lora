package lora

/*
#cgo CFLAGS: -I${SRCDIR}/include
#cgo linux  LDFLAGS: ${SRCDIR}/../../target/release/liblora_ffi.a -lm -ldl -lpthread
#cgo darwin LDFLAGS: ${SRCDIR}/../../target/release/liblora_ffi.a -framework Security -framework CoreFoundation

#include <stdlib.h>
#include "lora_ffi.h"
*/
import "C"

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"runtime"
	"runtime/debug"
	"sync"
	"unsafe"
)

// Options controls how a named database is opened.
type Options struct {
	// DatabaseDir is the directory that contains <databaseName>.loradb.
	// Empty means the current working directory.
	DatabaseDir string
}

// Database is a Lora graph database backed by the Rust engine. It is
// safe to share across goroutines; native handle access is protected
// by an internal RWMutex, while the Rust engine separately shares
// read-only work and serializes writes.
//
// Always call [Database.Close] when the database is no longer needed.
// A finalizer is installed as a safety net so a forgotten Close does
// not leak the native handle, but relying on the finalizer means the
// engine's memory can outlive its last Go reference arbitrarily long.
type Database struct {
	// mu serialises callers that need the native handle. Execute,
	// NodeCount, RelationshipCount, and Clear take the read lock so
	// they can proceed concurrently; Close takes the write lock
	// exactly once before freeing the handle. The RWMutex provides
	// the happens-before edge that keeps `handle` publication safe
	// without an atomic.
	mu     sync.RWMutex
	handle *C.LoraDatabase
}

// New allocates a Lora graph database.
//
// With no argument it returns a fresh in-memory database:
//
//	db, err := lora.New()
//
// Passing a database name opens or creates an archive-backed persistent
// database at <DatabaseDir>/<name>.loradb:
//
//	db, err := lora.New("app", lora.Options{DatabaseDir: "./data"})
func New(args ...any) (*Database, error) { return NewDatabase(args...) }

// NewDatabase is an alias for [New] kept for idiomatic Go naming
// parity with packages that expose Type-named constructors.
func NewDatabase(args ...any) (*Database, error) {
	var handle *C.LoraDatabase
	switch len(args) {
	case 0:
		status := C.lora_db_new(&handle)
		if status != C.LORA_STATUS_OK {
			return nil, &LoraError{Code: CodePanic, Message: fmt.Sprintf("lora_db_new returned status %d", int(status))}
		}
	case 1, 2:
		databaseName, ok := args[0].(string)
		if !ok {
			return nil, &LoraError{Code: CodeInvalidParams, Message: "database name must be a string"}
		}
		var options Options
		if len(args) == 2 {
			var ok bool
			options, ok = args[1].(Options)
			if !ok {
				return nil, &LoraError{Code: CodeInvalidParams, Message: "options must be lora.Options"}
			}
		}

		cDatabaseName := C.CString(databaseName)
		defer C.free(unsafe.Pointer(cDatabaseName))
		var cDatabaseDir *C.char
		if options.DatabaseDir != "" {
			cDatabaseDir = C.CString(options.DatabaseDir)
			defer C.free(unsafe.Pointer(cDatabaseDir))
		}
		var outError *C.char
		status := C.lora_db_new_named(&handle, cDatabaseName, cDatabaseDir, &outError)
		if status != C.LORA_STATUS_OK {
			defer func() {
				if outError != nil {
					C.lora_string_free(outError)
				}
			}()
			return nil, statusToError(int(status), outError)
		}
	default:
		return nil, &LoraError{
			Code:    CodeInvalidParams,
			Message: fmt.Sprintf("expected database name and optional lora.Options, got %d arguments", len(args)),
		}
	}
	db := &Database{handle: handle}
	// Safety net: if a caller forgets Close, the finalizer frees the
	// handle. This does not replace Close — the finalizer may run
	// arbitrarily late or not at all on process exit.
	runtime.SetFinalizer(db, func(d *Database) {
		_ = d.Close()
	})
	return db, nil
}

// Close releases the native database handle. Subsequent calls are
// no-ops. Close is safe to call concurrently with an in-flight
// Execute: the call will wait for the execute to finish before
// freeing the handle.
func (db *Database) Close() error {
	// Serialise with any other mutating call and, more importantly,
	// any goroutine that is currently inside an Execute and holding a
	// live pointer. Execute takes the read lock, Close takes the
	// write lock — they are mutually exclusive.
	db.mu.Lock()
	defer db.mu.Unlock()

	if db.handle == nil {
		return nil
	}
	runtime.SetFinalizer(db, nil)
	C.lora_db_free(db.handle)
	db.handle = nil
	return nil
}

// Execute runs a Cypher query with optional parameters and returns
// the result. See the package doc for the returned value model.
func (db *Database) Execute(query string, params Params) (*Result, error) {
	return db.ExecuteContext(context.Background(), query, params)
}

// Stream runs a query and returns an iterator over its rows. The current
// binding materializes the native result first, then exposes row-by-row
// consumption to Go callers.
func (db *Database) Stream(query string, params Params) (*RowIterator, error) {
	return db.StreamContext(context.Background(), query, params)
}

// StreamContext is the context-aware variant of [Database.Stream].
func (db *Database) StreamContext(ctx context.Context, query string, params Params) (*RowIterator, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	paramsJSON, err := encodeParams(params)
	if err != nil {
		return nil, &LoraError{Code: CodeInvalidParams, Message: err.Error()}
	}
	return db.stream(query, paramsJSON)
}

// ExecuteContext runs a Cypher query with optional parameters and
// cooperates with ctx cancellation. See the package doc for the
// caveat around mid-query cancellation — the native call cannot be
// interrupted through this binding, so a cancelled context only
// unblocks the caller; the query continues running and holds its Rust
// store lock until it finishes.
func (db *Database) ExecuteContext(ctx context.Context, query string, params Params) (*Result, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}

	paramsJSON, err := encodeParams(params)
	if err != nil {
		return nil, &LoraError{Code: CodeInvalidParams, Message: err.Error()}
	}

	done := make(chan executeResult, 1)
	go func() {
		r, err := db.execute(query, paramsJSON)
		done <- executeResult{r, err}
	}()

	select {
	case <-ctx.Done():
		// The Rust call is still in flight; the goroutine above will
		// publish to `done` (which is buffered) even after we return.
		return nil, ctx.Err()
	case out := <-done:
		return out.result, out.err
	}
}

type executeResult struct {
	result *Result
	err    error
}

func (db *Database) execute(query string, paramsJSON []byte) (*Result, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()

	if db.handle == nil {
		return nil, errClosed()
	}

	cQuery := C.CString(query)
	defer C.free(unsafe.Pointer(cQuery))

	var cParams *C.char
	if len(paramsJSON) > 0 {
		cParams = C.CString(string(paramsJSON))
		defer C.free(unsafe.Pointer(cParams))
	}

	var outResult *C.char
	var outError *C.char
	status := C.lora_db_execute_json(db.handle, cQuery, cParams, &outResult, &outError)

	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
			if outResult != nil {
				C.lora_string_free(outResult)
			}
		}()
		return nil, statusToError(int(status), outError)
	}

	defer C.lora_string_free(outResult)
	return decodeResult(C.GoString(outResult))
}

func (db *Database) stream(query string, paramsJSON []byte) (*RowIterator, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()

	if db.handle == nil {
		return nil, errClosed()
	}

	cQuery := C.CString(query)
	defer C.free(unsafe.Pointer(cQuery))

	var cParams *C.char
	if len(paramsJSON) > 0 {
		cParams = C.CString(string(paramsJSON))
		defer C.free(unsafe.Pointer(cParams))
	}

	var outStream *C.LoraQueryStream
	var outError *C.char
	status := C.lora_db_stream_open_json(db.handle, cQuery, cParams, &outStream, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		return nil, statusToError(int(status), outError)
	}

	it := &RowIterator{handle: outStream}
	runtime.SetFinalizer(it, func(it *RowIterator) {
		_ = it.Close()
	})
	return it, nil
}

// RowIterator pulls rows from a native Lora query stream.
type RowIterator struct {
	mu      sync.Mutex
	handle  *C.LoraQueryStream
	current Row
	err     error
}

// Columns returns the stream's projection columns.
func (it *RowIterator) Columns() ([]string, error) {
	it.mu.Lock()
	defer it.mu.Unlock()
	if it.handle == nil {
		if it.err != nil {
			return nil, it.err
		}
		return nil, nil
	}
	var outResult *C.char
	var outError *C.char
	status := C.lora_stream_columns_json(it.handle, &outResult, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		return nil, statusToError(int(status), outError)
	}
	defer C.lora_string_free(outResult)
	var columns []string
	if err := json.Unmarshal([]byte(C.GoString(outResult)), &columns); err != nil {
		return nil, err
	}
	return columns, nil
}

// Next advances the stream by one row.
func (it *RowIterator) Next() bool {
	it.mu.Lock()
	defer it.mu.Unlock()
	if it.handle == nil {
		return false
	}
	var outResult *C.char
	var outError *C.char
	status := C.lora_stream_next_json(it.handle, &outResult, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
		}()
		it.err = statusToError(int(status), outError)
		C.lora_stream_free(it.handle)
		it.handle = nil
		return false
	}
	if outResult == nil {
		C.lora_stream_free(it.handle)
		it.handle = nil
		runtime.SetFinalizer(it, nil)
		return false
	}
	defer C.lora_string_free(outResult)
	row, err := decodeRow(C.GoString(outResult))
	if err != nil {
		it.err = err
		C.lora_stream_free(it.handle)
		it.handle = nil
		return false
	}
	it.current = row
	return true
}

// Row returns the current row. Call after Next returns true.
func (it *RowIterator) Row() Row {
	it.mu.Lock()
	defer it.mu.Unlock()
	return it.current
}

// Err returns the first error observed while streaming.
func (it *RowIterator) Err() error {
	it.mu.Lock()
	defer it.mu.Unlock()
	return it.err
}

// Close releases the native stream. Closing before exhaustion rolls back a
// mutating stream.
func (it *RowIterator) Close() error {
	it.mu.Lock()
	defer it.mu.Unlock()
	if it.handle != nil {
		C.lora_stream_free(it.handle)
		it.handle = nil
	}
	runtime.SetFinalizer(it, nil)
	return nil
}

// Transaction executes a statement batch inside one native transaction.
// Results are returned in statement order. If any statement fails, the
// native transaction rolls back all earlier writes in the batch.
func (db *Database) Transaction(statements []TransactionStatement, mode TransactionMode) ([]*Result, error) {
	return db.TransactionContext(context.Background(), statements, mode)
}

// TransactionContext is the context-aware variant of [Database.Transaction].
func (db *Database) TransactionContext(ctx context.Context, statements []TransactionStatement, mode TransactionMode) ([]*Result, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}

	statementsJSON, err := encodeStatements(statements)
	if err != nil {
		return nil, &LoraError{Code: CodeInvalidParams, Message: err.Error()}
	}

	done := make(chan transactionResult, 1)
	go func() {
		r, err := db.transaction(statementsJSON, mode)
		done <- transactionResult{r, err}
	}()

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case out := <-done:
		return out.results, out.err
	}
}

type transactionResult struct {
	results []*Result
	err     error
}

func (db *Database) transaction(statementsJSON []byte, mode TransactionMode) ([]*Result, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()

	if db.handle == nil {
		return nil, errClosed()
	}

	cStatements := C.CString(string(statementsJSON))
	defer C.free(unsafe.Pointer(cStatements))

	var cMode *C.char
	if mode != "" {
		cMode = C.CString(string(mode))
		defer C.free(unsafe.Pointer(cMode))
	}

	var outResult *C.char
	var outError *C.char
	status := C.lora_db_transaction_json(db.handle, cStatements, cMode, &outResult, &outError)
	if status != C.LORA_STATUS_OK {
		defer func() {
			if outError != nil {
				C.lora_string_free(outError)
			}
			if outResult != nil {
				C.lora_string_free(outResult)
			}
		}()
		return nil, statusToError(int(status), outError)
	}

	defer C.lora_string_free(outResult)
	return decodeTransactionResults(C.GoString(outResult))
}

// Clear drops every node and relationship. The call is constant-time
// and blocks until in-flight queries release their Rust store locks.
func (db *Database) Clear() error {
	db.mu.RLock()
	defer db.mu.RUnlock()

	if db.handle == nil {
		return errClosed()
	}
	status := C.lora_db_clear(db.handle)
	if status != C.LORA_STATUS_OK {
		return &LoraError{Code: CodePanic, Message: fmt.Sprintf("lora_db_clear returned status %d", int(status))}
	}
	return nil
}

// NodeCount returns the number of nodes currently in the graph.
func (db *Database) NodeCount() (uint64, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return 0, errClosed()
	}
	var n C.uint64_t
	status := C.lora_db_node_count(db.handle, &n)
	if status != C.LORA_STATUS_OK {
		return 0, &LoraError{Code: CodePanic, Message: fmt.Sprintf("lora_db_node_count returned status %d", int(status))}
	}
	return uint64(n), nil
}

// RelationshipCount returns the number of relationships currently in
// the graph.
func (db *Database) RelationshipCount() (uint64, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.handle == nil {
		return 0, errClosed()
	}
	var n C.uint64_t
	status := C.lora_db_relationship_count(db.handle, &n)
	if status != C.LORA_STATUS_OK {
		return 0, &LoraError{Code: CodePanic, Message: fmt.Sprintf("lora_db_relationship_count returned status %d", int(status))}
	}
	return uint64(n), nil
}

func errClosed() error {
	return &LoraError{Code: CodeLoraError, Message: "database is closed"}
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

var (
	versionOnce  sync.Once
	versionValue string
)

// Version returns the version of the bundled lora-ffi library, read
// from the Go module build info when available, and otherwise from
// the lora_version() FFI call. The returned string is memoised.
func Version() string {
	versionOnce.Do(func() {
		if info, ok := debug.ReadBuildInfo(); ok {
			for _, dep := range info.Deps {
				if dep.Path == "github.com/lora-db/lora/crates/lora-go" && dep.Version != "" {
					versionValue = dep.Version
					return
				}
			}
		}
		versionValue = C.GoString(C.lora_version())
	})
	return versionValue
}

// ---------------------------------------------------------------------------
// Marshalling
// ---------------------------------------------------------------------------

func encodeParams(params Params) ([]byte, error) {
	if len(params) == 0 {
		return nil, nil
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	// Avoid JSON's default `<>& → <…` escaping; Cypher tolerates
	// the real characters in string literals and the FFI is
	// length-delimited, so the escaped form is just noise on the wire.
	enc.SetEscapeHTML(false)
	if err := enc.Encode(params); err != nil {
		return nil, err
	}
	// `json.Encoder` trails a newline; the FFI accepts it but strip
	// for cleanliness.
	out := buf.Bytes()
	if len(out) > 0 && out[len(out)-1] == '\n' {
		out = out[:len(out)-1]
	}
	return out, nil
}

func encodeStatements(statements []TransactionStatement) ([]byte, error) {
	if len(statements) == 0 {
		return []byte("[]"), nil
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(statements); err != nil {
		return nil, err
	}
	out := buf.Bytes()
	if len(out) > 0 && out[len(out)-1] == '\n' {
		out = out[:len(out)-1]
	}
	return out, nil
}

// decodeResult turns the FFI JSON payload into a Result. Uses
// json.Number so int64 parameters round-trip through Cypher without
// silently becoming float64.
func decodeResult(raw string) (*Result, error) {
	dec := json.NewDecoder(bytes.NewReader([]byte(raw)))
	dec.UseNumber()
	var wire struct {
		Columns []string          `json:"columns"`
		Rows    []json.RawMessage `json:"rows"`
	}
	if err := dec.Decode(&wire); err != nil {
		return nil, fmt.Errorf("lora: decode result envelope: %w", err)
	}

	rows := make([]Row, 0, len(wire.Rows))
	for i, rawRow := range wire.Rows {
		rd := json.NewDecoder(bytes.NewReader(rawRow))
		rd.UseNumber()
		var raw map[string]any
		if err := rd.Decode(&raw); err != nil {
			return nil, fmt.Errorf("lora: decode row %d: %w", i, err)
		}
		rows = append(rows, normalizeMap(raw))
	}
	return &Result{Columns: wire.Columns, Rows: rows}, nil
}

func decodeTransactionResults(raw string) ([]*Result, error) {
	dec := json.NewDecoder(bytes.NewReader([]byte(raw)))
	dec.UseNumber()
	var envelopes []json.RawMessage
	if err := dec.Decode(&envelopes); err != nil {
		return nil, fmt.Errorf("lora: decode transaction result array: %w", err)
	}
	results := make([]*Result, 0, len(envelopes))
	for i, envelope := range envelopes {
		result, err := decodeResult(string(envelope))
		if err != nil {
			return nil, fmt.Errorf("lora: decode transaction result %d: %w", i, err)
		}
		results = append(results, result)
	}
	return results, nil
}

func decodeRow(raw string) (Row, error) {
	dec := json.NewDecoder(bytes.NewReader([]byte(raw)))
	dec.UseNumber()
	var row map[string]any
	if err := dec.Decode(&row); err != nil {
		return nil, fmt.Errorf("lora: decode stream row: %w", err)
	}
	return normalizeMap(row), nil
}

// normalizeMap / normalizeSlice / normalize walk the decoded structure
// and convert json.Number values to int64 when possible, or float64
// otherwise. This keeps the "primitives as Go natives" contract even
// though UseNumber() hands back json.Number placeholders.
func normalize(v any) any {
	switch x := v.(type) {
	case map[string]any:
		return normalizeMap(x)
	case []any:
		return normalizeSlice(x)
	case json.Number:
		if i, err := x.Int64(); err == nil {
			return i
		}
		if f, err := x.Float64(); err == nil {
			return f
		}
		// Shouldn't happen given UseNumber() only emits numeric text.
		return x.String()
	default:
		return v
	}
}

func normalizeMap(m map[string]any) map[string]any {
	out := make(map[string]any, len(m))
	for k, v := range m {
		out[k] = normalize(v)
	}
	return out
}

func normalizeSlice(s []any) []any {
	out := make([]any, len(s))
	for i, v := range s {
		out[i] = normalize(v)
	}
	return out
}

// statusToError maps a non-OK FFI status plus its error string into
// a LoraError with the right Code.
func statusToError(status int, outError *C.char) error {
	payload := ""
	if outError != nil {
		payload = C.GoString(outError)
	}
	switch status {
	case C.LORA_STATUS_LORA_ERROR:
		return parseLoraError(payload, CodeLoraError)
	case C.LORA_STATUS_INVALID_PARAMS:
		return parseLoraError(payload, CodeInvalidParams)
	case C.LORA_STATUS_NULL_POINTER:
		return &LoraError{Code: CodePanic, Message: "null pointer passed to Lora FFI"}
	case C.LORA_STATUS_INVALID_UTF8:
		return parseLoraError(payload, CodeInvalidParams)
	case C.LORA_STATUS_PANIC:
		return parseLoraError(payload, CodePanic)
	default:
		return &LoraError{Code: CodeUnknown, Message: fmt.Sprintf("unknown FFI status %d: %s", status, payload)}
	}
}
