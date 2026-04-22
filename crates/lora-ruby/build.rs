fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `rb-sys-env` discovers the active Ruby's ABI (include paths, link
    // flags, whether to use dynamic_lookup on macOS, etc.) and exports
    // them as `cargo:rustc-link-*` / `cargo:rerun-if-*` instructions so
    // the cdylib links cleanly against libruby at load time.
    //
    // This is what lets `cargo check -p lora-ruby` work as a regular Rust
    // crate (via the `rlib` crate-type) without a full `rake compile`,
    // while the real shipped artefact is built through rb-sys' mkmf.
    let _ = rb_sys_env::activate()?;
    Ok(())
}
