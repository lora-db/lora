//! Re-export of the shared binary result-buffer encoder.
//!
//! The actual format and the Rust-side encoder live in
//! [`lora_binding_buffer`] so every binding (Node, WASM, Go via lora-ffi)
//! produces byte-for-byte identical buffers and the language-side
//! decoders only have to track one wire format.

pub use lora_binding_buffer::{encode_query_rows, encode_rows};
