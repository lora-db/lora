fn main() {
    // When building the Python extension module with `pyo3`'s
    // `extension-module` feature, libpython is not linked at build time —
    // symbols are resolved at runtime by the Python interpreter that loads
    // the `.so`/`.dylib`. On macOS, the linker still needs to be told to
    // allow those undefined references, which maturin does automatically
    // but plain `cargo build`/`cargo test` does not.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
