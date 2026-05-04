# frozen_string_literal: true

require_relative "test_helper"
require "fileutils"
require "tmpdir"

class TestSnapshot < Minitest::Test
  def test_encrypted_snapshot_path_roundtrip
    dir = Dir.mktmpdir("lora-ruby-snapshot-")
    path = File.join(dir, "secret.lsnap")
    encryption = {
      type: "password",
      keyId: "ruby-test",
      password: "open sesame",
      params: { memoryCostKib: 512, timeCost: 1, parallelism: 1 },
    }
    options = {
      compression: { format: "gzip", level: 1 },
      encryption: encryption,
    }

    source = LoraRuby::Database.create
    source.execute("CREATE (:Secret {name: 'Ada'})")
    meta = source.save_snapshot(path, options)
    assert_equal 1, meta["nodeCount"]

    target = LoraRuby::Database.create
    assert_raises(LoraRuby::QueryError) { target.load_snapshot(path) }

    meta = target.load_snapshot(path, { encryption: encryption })
    assert_equal 1, meta["nodeCount"]
    rows = target.execute("MATCH (n:Secret) RETURN n.name AS name")["rows"]
    assert_equal([{ "name" => "Ada" }], rows)
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end
end
