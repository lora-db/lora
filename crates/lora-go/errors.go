package lora

import (
	"errors"
	"strings"
)

// Code is a stable error discriminator attached to every [LoraError]
// surfaced from the Rust engine.
type Code string

const (
	// CodeLoraError covers parse, analyze, and execute failures. The
	// engine produced a structured error that isn't about the shape
	// of the arguments.
	CodeLoraError Code = "LORA_ERROR"
	// CodeInvalidParams signals that a parameter value could not be
	// mapped to a LoraValue (bad tagged object, unsupported number,
	// etc).
	CodeInvalidParams Code = "INVALID_PARAMS"
	// CodePanic signals a Rust panic was caught at the FFI boundary.
	// The binding translates this to a Go error; it does not crash
	// the process.
	CodePanic Code = "PANIC"
	// CodeUnknown is the catch-all for error messages that did not
	// carry a recognised prefix.
	CodeUnknown Code = "UNKNOWN"
)

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

// Is lets callers use errors.Is(err, &LoraError{Code: lora.CodeLoraError}).
// Two LoraErrors are considered equal for Is-matching if they have the
// same Code; Message is ignored for the match.
func (e *LoraError) Is(target error) bool {
	var t *LoraError
	if !errors.As(target, &t) {
		return false
	}
	return t.Code == e.Code
}

// parseLoraError turns an "CODE: message" payload returned by the FFI
// into a typed LoraError. Fallback statuses (null pointer, UTF-8 issue,
// panic) map onto CodePanic / CodeUnknown so callers still get a
// structured LoraError without having to special-case a raw string.
func parseLoraError(payload string, fallback Code) *LoraError {
	for _, code := range []Code{CodeLoraError, CodeInvalidParams} {
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
