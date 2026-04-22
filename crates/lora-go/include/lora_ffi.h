/*
 * lora_ffi.h — C ABI for the Lora in-memory graph database.
 *
 * This header is hand-written to match the `#[no_mangle] extern "C" fn`
 * surface in `crates/lora-ffi/src/lib.rs`. Changes on the Rust side must
 * be reflected here.
 *
 * Ownership
 * ---------
 * - `LoraDatabase *` is opaque. Allocate with `lora_db_new`; release
 *   with `lora_db_free`. Passing the same handle after `lora_db_free`
 *   is undefined behaviour.
 * - Heap strings (`char *` out-parameters) are Rust-allocated and must
 *   be released via `lora_string_free`. Do not call `free()`.
 * - `lora_version()` returns a process-lifetime static string and must
 *   NOT be freed.
 * - Input `const char *` arguments are borrowed; Rust copies what it
 *   needs before returning.
 *
 * Threading
 * ---------
 * A single `LoraDatabase` is safe to use from multiple threads;
 * concurrent queries serialise on an internal mutex. `lora_db_free`
 * must not race with any other call on the same handle.
 *
 * Panics
 * ------
 * Every function wraps its body in `std::panic::catch_unwind`; a Rust
 * panic cannot unwind into the caller. A recovered panic is reported
 * with `LORA_STATUS_PANIC` and an error string when applicable.
 */

#ifndef LORA_FFI_H
#define LORA_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* --- Status codes ----------------------------------------------------- */

#define LORA_STATUS_OK              0
#define LORA_STATUS_LORA_ERROR      1
#define LORA_STATUS_INVALID_PARAMS  2
#define LORA_STATUS_NULL_POINTER    3
#define LORA_STATUS_INVALID_UTF8    4
#define LORA_STATUS_PANIC           5

/* --- Types ------------------------------------------------------------ */

typedef struct LoraDatabase LoraDatabase;

/* --- Version ---------------------------------------------------------- */

/* Returns a process-lifetime static string. Do NOT free. */
const char *lora_version(void);

/* --- Lifecycle -------------------------------------------------------- */

/* Allocates a new in-memory database. On success writes a handle into
 * `*out_db` and returns LORA_STATUS_OK. The handle must be released with
 * lora_db_free. */
int lora_db_new(LoraDatabase **out_db);

/* Frees a handle returned by lora_db_new. Null is a no-op. */
void lora_db_free(LoraDatabase *db);

/* --- Execute ---------------------------------------------------------- */

/* Executes `query` with optional JSON-encoded parameters.
 *
 * params_json may be:
 *   - NULL                    => no params
 *   - ""                      => no params
 *   - "null"                  => no params
 *   - a JSON object literal   => parsed into the params map
 *
 * On LORA_STATUS_OK, `*out_result` is set to a NUL-terminated JSON
 * payload of the form `{"columns":[…],"rows":[…]}`. Every other status
 * sets `*out_error` to a NUL-terminated string starting with one of
 * `LORA_ERROR: ` or `INVALID_PARAMS: `. Caller frees both with
 * lora_string_free. */
int lora_db_execute_json(
    LoraDatabase *db,
    const char *query,
    const char *params_json,
    char **out_result,
    char **out_error);

/* --- Clear / counts --------------------------------------------------- */

int lora_db_clear(LoraDatabase *db);
int lora_db_node_count(LoraDatabase *db, uint64_t *out);
int lora_db_relationship_count(LoraDatabase *db, uint64_t *out);

/* --- String release --------------------------------------------------- */

/* Frees a heap `char *` returned via one of the `*_out_*` parameters.
 * Null is a no-op. Passing a pointer NOT returned by this library is
 * undefined. */
void lora_string_free(char *s);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* LORA_FFI_H */
