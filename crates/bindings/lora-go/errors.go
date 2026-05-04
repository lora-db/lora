package lora

import (
	"errors"
	"strings"
)

// Code is a stable error discriminator attached to every [LoraError]
// surfaced from the Rust engine. The wire string matches the
// `LORA_*` codes in `lora_database::LoraErrorCode` exactly; matching
// on a Code value is the supported way to route errors in user code.
type Code string

const (
	// -------- Client errors --------

	// CodeParse — Cypher syntax could not be parsed.
	CodeParse Code = "LORA_PARSE"
	// CodeSemantic — Cypher analysis (unknown variable, label, function,
	// type mismatch).
	CodeSemantic Code = "LORA_SEMANTIC"
	// CodeInvalidParams — a parameter value passed by the caller could
	// not be coerced into a Lora value.
	CodeInvalidParams Code = "LORA_INVALID_PARAMS"
	// CodeReadOnly — a mutating statement was issued in a read-only
	// context.
	CodeReadOnly Code = "LORA_READ_ONLY"
	// CodeNotFound — a named entity (database, label, key) does not exist.
	CodeNotFound Code = "LORA_NOT_FOUND"
	// CodeConstraint — a precondition (e.g. delete-with-relationships)
	// is not satisfied.
	CodeConstraint Code = "LORA_CONSTRAINT"
	// CodeInvalidVector — a vector value failed dimension /
	// coordinate-type validation.
	CodeInvalidVector Code = "LORA_INVALID_VECTOR"
	// CodeTimeout — a query exceeded its cooperative deadline.
	CodeTimeout Code = "LORA_TIMEOUT"
	// CodeDatabaseName — a logical database name violates the
	// portable-path rules.
	CodeDatabaseName Code = "LORA_DATABASE_NAME"
	// CodeConfig — required parameters are missing or malformed (CLI /
	// config flags).
	CodeConfig Code = "LORA_CONFIG"

	// -------- Server errors --------

	// CodeIO — I/O failure outside the WAL / snapshot boundaries.
	CodeIO Code = "LORA_IO"
	// CodeWalCorruption — WAL record was truncated, mis-CRC'd, or
	// otherwise unreadable.
	CodeWalCorruption Code = "LORA_WAL_CORRUPTION"
	// CodeWalPoisoned — the WAL is poisoned and no longer accepts
	// durable writes.
	CodeWalPoisoned Code = "LORA_WAL_POISONED"
	// CodeSnapshotCodec — snapshot codec failure (bad magic, version,
	// checksum, …).
	CodeSnapshotCodec Code = "LORA_SNAPSHOT_CODEC"
	// CodeSnapshotCrypto — snapshot encryption / decryption / KDF
	// failure.
	CodeSnapshotCrypto Code = "LORA_SNAPSHOT_CRYPTO"
	// CodeInternal — last-resort fallback when the engine cannot
	// classify the failure.
	CodeInternal Code = "LORA_INTERNAL"

	// -------- Binding-side fallbacks --------

	// CodePanic signals a Rust panic was caught at the FFI boundary.
	// The binding translates this to a Go error; it does not crash
	// the process.
	CodePanic Code = "LORA_PANIC"
	// CodeUnknown is the catch-all for error messages that did not
	// carry a recognised prefix.
	CodeUnknown Code = "UNKNOWN"

	// CodeLoraError — Deprecated: use the precise codes above.
	// Retained as a constant for callers that still match against the
	// pre-0.7 wire string. New code should branch on [Code] equality.
	CodeLoraError Code = "LORA_ERROR"
)

// allKnownCodes lists every code prefix the FFI may emit, used by
// parseLoraError to extract the discriminator.
var allKnownCodes = []Code{
	CodeParse,
	CodeSemantic,
	CodeInvalidParams,
	CodeReadOnly,
	CodeNotFound,
	CodeConstraint,
	CodeInvalidVector,
	CodeTimeout,
	CodeDatabaseName,
	CodeConfig,
	CodeIO,
	CodeWalCorruption,
	CodeWalPoisoned,
	CodeSnapshotCodec,
	CodeSnapshotCrypto,
	CodeInternal,
	CodePanic,
	CodeLoraError,
}

// LoraError is the error type returned by every method on [Database].
// The [Code] field always parses to one of the constants above; the
// Message carries the engine's human-readable description minus the
// discriminator prefix.
type LoraError struct {
	Code    Code
	Message string
}

// Error returns the error in the canonical "CODE: message" form used by
// every LoraDB binding.
func (e *LoraError) Error() string {
	return string(e.Code) + ": " + e.Message
}

// Is lets callers use errors.Is(err, &LoraError{Code: lora.CodeTimeout}).
// Two LoraErrors are considered equal for Is-matching if they have the
// same Code; Message is ignored for the match.
func (e *LoraError) Is(target error) bool {
	var t *LoraError
	if !errors.As(target, &t) {
		return false
	}
	return t.Code == e.Code
}

// IsClient reports whether the error came from caller-side input
// (parse, semantic, invalid params, …) versus engine-side conditions
// (I/O, WAL corruption, …). Mirrors LoraErrorCategory in Rust.
func (e *LoraError) IsClient() bool {
	switch e.Code {
	case CodeParse, CodeSemantic, CodeInvalidParams, CodeReadOnly,
		CodeNotFound, CodeConstraint, CodeInvalidVector, CodeTimeout,
		CodeDatabaseName, CodeConfig:
		return true
	default:
		return false
	}
}

// parseLoraError turns a "CODE: message" payload returned by the FFI
// into a typed LoraError. Fallback statuses (null pointer, UTF-8 issue,
// panic) map onto CodePanic / CodeUnknown so callers still get a
// structured LoraError without having to special-case a raw string.
func parseLoraError(payload string, fallback Code) *LoraError {
	for _, code := range allKnownCodes {
		prefix := string(code) + ": "
		if strings.HasPrefix(payload, prefix) {
			return &LoraError{Code: code, Message: payload[len(prefix):]}
		}
	}
	if payload == "" {
		return &LoraError{Code: fallback, Message: "(no error message)"}
	}
	return &LoraError{Code: fallback, Message: payload}
}
