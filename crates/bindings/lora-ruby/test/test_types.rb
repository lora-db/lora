# frozen_string_literal: true

require_relative "test_helper"

class TestTypes < Minitest::Test
  def setup
    @db = LoraRuby::Database.create
  end

  # ---- temporal ----------------------------------------------------

  def test_tagged_date_value
    @db.execute("CREATE (:E {d: date('2025-03-14')})")
    rows = @db.execute("MATCH (n:E) RETURN n.d AS d")["rows"]
    d = rows[0]["d"]
    assert LoraRuby.temporal?(d)
    assert_equal({ "kind" => "date", "iso" => "2025-03-14" }, d)
  end

  def test_accepts_typed_temporal_params
    @db.execute(
      "CREATE (:E {on: $d, span: $dur})",
      { d: LoraRuby.date("2025-01-15"), dur: LoraRuby.duration("P1M") },
    )
    rows = @db.execute("MATCH (n:E) RETURN n.on AS on, n.span AS span")["rows"]
    assert_equal({ "kind" => "date", "iso" => "2025-01-15" }, rows[0]["on"])
    assert_equal({ "kind" => "duration", "iso" => "P1M" }, rows[0]["span"])
  end

  def test_invalid_temporal_param_raises_invalid_params_error
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $d AS d", { d: { "kind" => "date", "iso" => "not-a-date" } })
    end
  end

  # ---- spatial -----------------------------------------------------

  def test_cartesian_2d_roundtrip
    @db.execute("CREATE (:P {c: $c})", { c: LoraRuby.cartesian(1.5, 2.5) })
    rows = @db.execute("MATCH (n:P) RETURN n.c AS c")["rows"]
    c = rows[0]["c"]
    assert LoraRuby.point?(c)
    assert_equal 7203,          c["srid"]
    assert_equal "cartesian",   c["crs"]
    assert_in_delta 1.5, c["x"]
    assert_in_delta 2.5, c["y"]
    refute c.key?("z")
    refute c.key?("longitude")
  end

  def test_wgs84_2d_roundtrip
    @db.execute("CREATE (:P {g: $g})", { g: LoraRuby.wgs84(4.9, 52.37) })
    rows = @db.execute("MATCH (n:P) RETURN n.g AS g")["rows"]
    g = rows[0]["g"]
    assert LoraRuby.point?(g)
    assert_equal 4326,          g["srid"]
    assert_equal "WGS-84-2D",   g["crs"]
    assert_in_delta 4.9,   g["longitude"]
    assert_in_delta 52.37, g["latitude"]
    refute g.key?("z")
    refute g.key?("height")
  end

  def test_cartesian_3d_roundtrip
    @db.execute(
      "CREATE (:P3 {c: $c})",
      { c: LoraRuby.cartesian_3d(1.0, 2.0, 3.0) },
    )
    rows = @db.execute("MATCH (n:P3) RETURN n.c AS c")["rows"]
    c = rows[0]["c"]
    assert LoraRuby.point?(c)
    assert_equal 9157,            c["srid"]
    assert_equal "cartesian-3D",  c["crs"]
    assert_in_delta 3.0, c["z"]
    refute c.key?("longitude")
  end

  def test_wgs84_3d_roundtrip
    @db.execute(
      "CREATE (:P3 {g: $g})",
      { g: LoraRuby.wgs84_3d(4.89, 52.37, 15.0) },
    )
    rows = @db.execute("MATCH (n:P3) RETURN n.g AS g")["rows"]
    g = rows[0]["g"]
    assert LoraRuby.point?(g)
    assert_equal 4979,          g["srid"]
    assert_equal "WGS-84-3D",   g["crs"]
    assert_in_delta 4.89,  g["longitude"]
    assert_in_delta 52.37, g["latitude"]
    assert_in_delta 15.0,  g["height"]
  end

  def test_point_from_cypher_constructor_round_trips
    # 3D points built inside Cypher also emit the canonical external
    # shape — same contract as lora-python's equivalent test.
    rows = @db.execute("RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p")["rows"]
    p = rows[0]["p"]
    assert LoraRuby.point?(p)
    assert_equal({
                   "kind" => "point",
                   "srid" => 9157,
                   "crs"  => "cartesian-3D",
                   "x"    => 1.0,
                   "y"    => 2.0,
                   "z"    => 3.0,
                 }, p)
  end

  # ---- guards ------------------------------------------------------

  def test_guards_accept_symbol_keyed_hashes
    # The extension emits string keys; guards should still work if a
    # caller has built a tagged hash by hand with symbol keys.
    assert LoraRuby.node?(kind: "node", id: 1)
    assert LoraRuby.point?(kind: "point", srid: 7203)
    assert LoraRuby.temporal?(kind: "duration", iso: "P1M")
  end

  def test_guards_reject_non_hashes
    refute LoraRuby.node?(nil)
    refute LoraRuby.point?("not a hash")
    refute LoraRuby.temporal?([1, 2, 3])
  end
end
