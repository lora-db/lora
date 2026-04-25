# frozen_string_literal: true

module LoraRuby
  # Gem version — kept in lockstep with the workspace Cargo.toml version
  # by `scripts/sync-versions.mjs`. The native extension also defines
  # `LoraRuby::VERSION` (from Cargo's `CARGO_PKG_VERSION`); we pre-seed
  # the constant here so `require "lora_ruby/version"` works during
  # `gem build` / `bundle install` before the `.so` has been compiled.
  #
  # Guard against redefinition so re-requiring this file (or loading
  # both paths) doesn't emit a "warning: already initialized constant"
  # when the native extension loads second with the identical value.
  VERSION = "0.4.0" unless const_defined?(:VERSION)
end
