# frozen_string_literal: true

# rb-sys provides a drop-in replacement for `create_makefile` that shells
# out to cargo to build the `cdylib` declared in ../../Cargo.toml. The
# path it writes the native library to is determined by the argument —
# `lora_ruby/lora_ruby` places the final artefact at
# `lib/lora_ruby/lora_ruby.{so,bundle,dll}`, which is what
# `require "lora_ruby/lora_ruby"` looks for.
require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("lora_ruby/lora_ruby") do |r|
  # Always build in release profile — the Rust debug profile is ~10x
  # slower for graph work and there's no interactive-debug value in a
  # gem extension.
  r.profile = :release
end
