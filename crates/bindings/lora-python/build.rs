fn main() {
    // When building the Python extension module with `pyo3`'s
    // `extension-module` feature, libpython is not linked at build time —
    // symbols are resolved at runtime by the Python interpreter that loads
    // the `.so`/`.dylib`. On macOS, the linker still needs to be told to
    // allow those undefined references, which maturin does automatically
    // but plain `cargo build`/`cargo test` does not.
    // `rustc-link-arg` applies to bins, tests, benches, examples, and
    // cdylib artifacts — all the targets that go through the linker. This
    // covers the cdylib import case and also the `cargo test` lib-test
    // executable, which would otherwise fail with undefined `_Py*` symbols.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-undefined");
        println!("cargo:rustc-link-arg=dynamic_lookup");
    }
}
