# frozen_string_literal: true

require_relative "lib/lora_ruby/version"

Gem::Specification.new do |spec|
  spec.name        = "lora-ruby"
  spec.version     = LoraRuby::VERSION
  spec.authors     = ["LoraDB, Inc."]
  spec.summary     = "Ruby bindings for the Lora in-memory graph database"
  spec.description =
    "Ruby bindings for the Lora in-memory graph database. Exposes the " \
    "embedded Rust engine as a native extension via Magnus/rb-sys, so " \
    "queries run in-process without a separate server."

  spec.license     = "BUSL-1.1"
  spec.homepage    = "https://github.com/lora-db/lora"
  spec.metadata    = {
    "homepage_uri"      => "https://github.com/lora-db/lora",
    "source_code_uri"   => "https://github.com/lora-db/lora/tree/main/crates/lora-ruby",
    "bug_tracker_uri"   => "https://github.com/lora-db/lora/issues",
    "documentation_uri" => "https://github.com/lora-db/lora/blob/main/crates/lora-ruby/README.md",
    "rubygems_mfa_required" => "true",
  }

  spec.required_ruby_version     = ">= 3.0"
  spec.required_rubygems_version = ">= 3.3.11" # rb-sys cross requires modern rubygems

  # Source layout — rb-sys' convention: `extconf.rb` and `Cargo.toml`
  # sit side by side at the gem root; `rb_sys/mkmf` uses the cargo
  # manifest directory to locate both. This keeps the Cargo workspace
  # layout clean (each crate is a plain cargo package at
  # `crates/<name>/`) without a separate `ext/` subdirectory.
  spec.require_paths = ["lib"]
  spec.extensions    = ["extconf.rb"]

  # Files shipped in the gem. Keep this narrow — the Cargo workspace
  # parent and root LICENSE are pulled in via the release workflow
  # (`copy-license` step), and target/ is emphatically excluded.
  spec.files = Dir[
    "lib/**/*.rb",
    "src/**/*.rs",
    "extconf.rb",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
    "LICENSE",
    "README.md",
  ].reject { |f| f.start_with?("target/", "tmp/") }

  # Runtime deps. `rb_sys` is the Ruby-side companion to the `rb-sys`
  # crate; it locates / builds the native extension at install time.
  spec.add_dependency "rb_sys", "~> 0.9"

  spec.add_development_dependency "rake", "~> 13.2"
  spec.add_development_dependency "rake-compiler", "~> 1.2"
  spec.add_development_dependency "minitest", "~> 5.20"
end
