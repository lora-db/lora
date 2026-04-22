# frozen_string_literal: true

require_relative "test_helper"

class TestParams < Minitest::Test
  def setup
    @db = LoraRuby::Database.create
  end

  def test_scalar_types
    @db.execute(
      "CREATE (:Item {name: $n, qty: $q, active: $a, score: $s})",
      { n: "widget", q: 42, a: true, s: 1.5 },
    )
    r = @db.execute(
      "MATCH (i:Item) RETURN i.name AS name, i.qty AS qty, i.active AS active, i.score AS score",
    )
    assert_equal [{ "name" => "widget", "qty" => 42, "active" => true, "score" => 1.5 }],
                 r["rows"]
  end

  def test_nil_params_allowed
    r = @db.execute("RETURN $x AS x", { x: nil })
    assert_nil r["rows"][0]["x"]
  end

  def test_string_and_symbol_keys_both_work
    @db.execute("CREATE (:S {name: $name})", { "name" => "strkey" })
    @db.execute("CREATE (:S {name: $name})", { name: "symkey" })
    r = @db.execute("MATCH (s:S) RETURN s.name AS name")
    names = r["rows"].map { |row| row["name"] }
    assert_includes names, "strkey"
    assert_includes names, "symkey"
  end

  def test_mixed_list_roundtrip
    @db.execute("CREATE (:N {xs: $xs})", { xs: [1, "two", true, nil] })
    rows = @db.execute("MATCH (n:N) RETURN n.xs AS xs")["rows"]
    assert_equal [1, "two", true, nil], rows[0]["xs"]
  end

  def test_nested_map_roundtrip
    @db.execute(
      "CREATE (:N {meta: $m})",
      { m: { a: 1, b: { c: "deep", d: [true, false] } } },
    )
    rows = @db.execute("MATCH (n:N) RETURN n.meta AS m")["rows"]
    # Output is always string-keyed regardless of input key type.
    assert_equal({ "a" => 1, "b" => { "c" => "deep", "d" => [true, false] } },
                 rows[0]["m"])
  end

  def test_symbol_param_value_round_trips_as_string
    r = @db.execute("RETURN $s AS s", { s: :hello })
    assert_equal "hello", r["rows"][0]["s"]
  end

  def test_invalid_param_container_raises
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $x AS x", "not-a-hash")
    end
  end

  def test_non_string_non_symbol_key_raises
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $x AS x", { 1 => "oops" })
    end
  end

  def test_unsupported_param_value_type_raises
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $x AS x", { x: Object.new })
    end
  end
end
