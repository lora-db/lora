# frozen_string_literal: true

require_relative "test_helper"
require "fileutils"
require "tmpdir"

class TestBasic < Minitest::Test
  def setup
    @db = LoraRuby::Database.create
  end

  def test_empty_match_returns_empty_rows
    r = @db.execute("MATCH (n) RETURN n")
    assert_equal [], r["rows"]
    assert_equal [], r["columns"]
  end

  def test_version_exposed_both_as_native_constant_and_from_ruby
    refute_nil LoraRuby::VERSION
    # matches semver shape used by scripts/sync-versions.mjs
    assert_match(/\A\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?\z/, LoraRuby::VERSION)
  end

  def test_create_and_return_node_with_properties
    @db.execute("CREATE (:Person {name: 'Alice', age: 30})")
    assert_equal 1, @db.node_count

    r = @db.execute("MATCH (n:Person) RETURN n")
    assert_equal 1, r["rows"].length
    n = r["rows"][0]["n"]
    assert LoraRuby.node?(n)
    # Node labels/properties are populated when the engine wires
    # richer node values through; the discriminator contract is the
    # part we pin here (kind + id keys always present).
    assert_equal "node", n["kind"]
    assert_kind_of Integer, n["id"]
  end

  def test_relationship_has_discriminator
    @db.execute("CREATE (:A {n:1})-[:R {w:2}]->(:B {n:3})")
    r = @db.execute("MATCH ()-[r:R]->() RETURN r")
    rel = r["rows"][0]["r"]
    assert LoraRuby.relationship?(rel)
    assert_equal "relationship", rel["kind"]
    assert_kind_of Integer, rel["id"]
  end

  def test_clear_empties_graph
    @db.execute("CREATE (:X), (:Y)-[:R]->(:Z)")
    assert_equal 3, @db.node_count
    assert_equal 1, @db.relationship_count

    @db.clear
    assert_equal 0, @db.node_count
    assert_equal 0, @db.relationship_count
  end

  def test_inspect_includes_counts
    @db.execute("CREATE (:X), (:Y)-[:R]->(:Z)")
    assert_equal "#<LoraRuby::Database nodes=3 relationships=1>", @db.inspect
  end

  def test_both_constructors_work
    a = LoraRuby::Database.create
    b = LoraRuby::Database.new
    assert_instance_of LoraRuby::Database, a
    assert_instance_of LoraRuby::Database, b
    assert_equal 0, a.node_count
    assert_equal 0, b.node_count
  end

  def test_named_database_persists_across_reopen
    dir = Dir.mktmpdir("lora-ruby-wal-")
    first = LoraRuby::Database.create("app", { database_dir: dir })
    first.execute(
      "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Grace'})",
    )
    first.close

    second = LoraRuby::Database.new("app", { database_dir: dir })
    assert_equal 2, second.node_count
    assert_equal 1, second.relationship_count
    assert_equal(
      [
        { "name" => "Ada" },
        { "name" => "Grace" },
      ],
      second.execute("MATCH (p:Person) RETURN p.name AS name ORDER BY name")["rows"],
    )
    second.close
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_relative_database_dir_path_works
    dir = Dir.mktmpdir("lora-ruby-relative-")
    Dir.chdir(dir) do
      first = LoraRuby::Database.create("app", { database_dir: "relative-wal" })
      first.execute("CREATE (:Session {value: 'ok'})")
      first.close

      second = LoraRuby::Database.create("app", { database_dir: "relative-wal" })
      assert_equal(
        [{ "value" => "ok" }],
        second.execute("MATCH (s:Session) RETURN s.value AS value")["rows"],
      )
      second.close
    end
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_invalid_wal_dir_raises_query_error
    dir = Dir.mktmpdir("lora-ruby-invalid-")
    path = File.join(dir, "wal-file")
    File.write(path, "not a directory")

    assert_raises(LoraRuby::QueryError) do
      LoraRuby::Database.create("app", { database_dir: path })
    end
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_managed_wal_snapshots_recover_snapshot_then_newer_wal
    dir = Dir.mktmpdir("lora-ruby-managed-snapshot-")
    wal_dir = File.join(dir, "wal")
    snapshot_dir = File.join(dir, "snapshots")

    first = LoraRuby::Database.open_wal(
      wal_dir,
      {
        snapshot_dir: snapshot_dir,
        snapshot_every_commits: 2,
      },
    )
    first.execute("CREATE (:Managed {id: 1})")
    first.execute("CREATE (:Managed {id: 2})")
    assert File.file?(File.join(snapshot_dir, "CURRENT"))
    first.execute("CREATE (:Managed {id: 3})")
    first.close

    second = LoraRuby::Database.open_wal(
      wal_dir,
      {
        snapshot_dir: snapshot_dir,
        snapshot_every_commits: 2,
      },
    )
    assert_equal(
      [{ "id" => 1 }, { "id" => 2 }, { "id" => 3 }],
      second.execute("MATCH (n:Managed) RETURN n.id AS id ORDER BY id")["rows"],
    )
    second.close
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_create_rejects_wal_options_for_memory_database
    dir = Dir.mktmpdir("lora-ruby-managed-snapshot-options-")

    error = assert_raises(LoraRuby::QueryError) do
      LoraRuby::Database.create(
        {
          wal_dir: File.join(dir, "wal"),
          snapshot_every_commits: 2,
        },
      )
    end
    assert_includes error.message, "open_wal"
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_managed_snapshot_options_require_snapshot_dir
    dir = Dir.mktmpdir("lora-ruby-managed-snapshot-options-")

    error = assert_raises(LoraRuby::QueryError) do
      LoraRuby::Database.open_wal(
        File.join(dir, "wal"),
        {
          snapshot_every_commits: 2,
        },
      )
    end
    assert_includes error.message, "snapshot_dir"
  ensure
    FileUtils.remove_entry(dir) if dir && File.exist?(dir)
  end

  def test_invalid_database_name_raises_query_error
    assert_raises(LoraRuby::QueryError) do
      LoraRuby::Database.create("../bad")
    end
  end

  def test_path_invariant
    @db.execute("CREATE (:A {n:1})-[:R]->(:B {n:2})")
    r = @db.execute("MATCH p = (:A)-[:R]->(:B) RETURN p")
    p = r["rows"][0]["p"]
    assert LoraRuby.path?(p)
    assert_equal p["nodes"].length, p["rels"].length + 1
  end

  def test_temporal_now_functions_work
    # date() / datetime() / ... no-arg forms use the wall clock; they
    # must not raise inside the extension.
    r = @db.execute(
      "RETURN date() AS d, datetime() AS dt, time() AS t, localdatetime() AS ldt, localtime() AS lt",
    )
    row = r["rows"][0]
    %w[d dt t ldt lt].each do |k|
      assert LoraRuby.temporal?(row[k]), "#{k} should be a tagged temporal hash"
    end
    assert row["d"]["iso"][0, 4].to_i >= 2024
  end
end
