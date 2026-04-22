# frozen_string_literal: true

# Public entry point for the gem. Users `require "lora_ruby"` which
# loads the native extension and re-exports the public API.
#
#   require "lora_ruby"
#
#   db = LoraRuby::Database.create
#   db.execute("CREATE (:Person {name: $n})", { n: "Alice" })
#   result = db.execute("MATCH (n:Person) RETURN n.name AS name")
#   result["rows"] # => [{"name" => "Alice"}]

require_relative "lora_ruby/version"

# Load the native extension. rb-sys builds it under the gem's extension
# directory and installs it to lib/lora_ruby/lora_ruby.{so,bundle,dll}
# from extconf.rb's create_rust_makefile("lora_ruby/lora_ruby").
require "lora_ruby/lora_ruby"

require_relative "lora_ruby/types"

module LoraRuby
  # Top-level sugar so callers can write `LoraRuby.cartesian(...)`
  # without having to say `LoraRuby::Types.cartesian(...)`. Same
  # trade-off `lora_python.types` makes.
  #
  # We explicitly re-export each method rather than `extend Types`
  # because `Types`' methods are declared with `module_function`,
  # which makes them public module methods on `Types` itself but
  # private instance methods — extending would copy them over as
  # private singletons on `LoraRuby`, which isn't the intended UX.
  %i[
    date time localtime datetime localdatetime duration
    cartesian cartesian_3d wgs84 wgs84_3d
    node? relationship? path? point? temporal?
  ].each do |m|
    define_singleton_method(m) do |*args, **kwargs|
      if kwargs.empty?
        Types.public_send(m, *args)
      else
        Types.public_send(m, *args, **kwargs)
      end
    end
  end
end
