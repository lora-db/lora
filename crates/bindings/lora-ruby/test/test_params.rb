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

  def test_vector_return_has_tagged_shape
    r = @db.execute("RETURN [1,2,3]::VECTOR<INTEGER>(3) AS v")
    v = r["rows"][0]["v"]
    assert LoraRuby.vector?(v)
    assert_equal 3, v["dimension"]
    assert_equal "INTEGER", v["coordinateType"]
    assert_equal [1, 2, 3], v["values"]
  end

  def test_vector_parameter_round_trip
    vec = LoraRuby.vector([0.1, 0.2, 0.3], 3, "FLOAT32")
    r = @db.execute("RETURN $v AS v", { v: vec })
    v = r["rows"][0]["v"]
    assert LoraRuby.vector?(v)
    assert_equal "FLOAT32", v["coordinateType"]
    assert_equal 3, v["dimension"]
  end

  def test_vector_parameter_in_similarity_function
    q = LoraRuby.vector([1.0, 0.0, 0.0], 3, "FLOAT32")
    r = @db.execute(
      "RETURN vector.similarity([1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3), $q) AS s",
      { q: q },
    )
    assert_in_delta 1.0, r["rows"][0]["s"], 1e-6
  end

  def test_vector_parameter_stored_as_node_property
    vec = LoraRuby.vector([1, 2, 3], 3, "INTEGER8")
    @db.execute("CREATE (:Doc {id: 1, embedding: $e})", { e: vec })
    r = @db.execute("MATCH (d:Doc) RETURN d.embedding AS e")
    stored = r["rows"][0]["e"]
    assert LoraRuby.vector?(stored)
    assert_equal "INTEGER8", stored["coordinateType"]
    assert_equal [1, 2, 3], stored["values"]
  end

  def test_malformed_vector_param_raises_invalid_params
    bad = {
      "kind" => "vector",
      "dimension" => 2,
      "coordinateType" => "FLOAT32",
      "values" => [1.0, "oops"],
    }
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $v AS v", { v: bad })
    end
  end

  def test_vector_param_unknown_coord_type_raises_invalid_params
    bad = {
      "kind" => "vector",
      "dimension" => 2,
      "coordinateType" => "BIGINT",
      "values" => [1, 2],
    }
    assert_raises(LoraRuby::InvalidParamsError) do
      @db.execute("RETURN $v AS v", { v: bad })
    end
  end

  def test_vector_predicate_rejects_non_vectors
    refute LoraRuby.vector?(nil)
    refute LoraRuby.vector?([1, 2, 3])
    refute LoraRuby.vector?({})
    refute LoraRuby.vector?({ "kind" => "node", "id" => 1 })
    refute LoraRuby.vector?(42)
    refute LoraRuby.vector?("vector")
  end

  # -- v0.10 namespaced built-ins ---------------------------------------------
  #
  # `<namespace>.<operation>` is the canonical surface; Cypher's historical
  # spellings (`head`, `coalesce`, `toLower`, …) resolve through the
  # analyzer's alias table to the same canonical implementations.

  def test_namespaced_string_list_math_value_builtins
    r = @db.execute(
      <<~CYPHER,
        RETURN string.upper('hello')              AS upper,
               string.lower('WORLD')              AS lower,
               list.first([10, 20, 30])           AS head,
               math.clamp($x, 0, 100)             AS bounded,
               value.coalesce($maybe, 'fallback') AS pick
      CYPHER
      { x: 250, maybe: nil },
    )
    row = r["rows"][0]
    assert_equal "HELLO", row["upper"]
    assert_equal "world", row["lower"]
    assert_equal 10, row["head"]
    assert_equal 100, row["bounded"]
    assert_equal "fallback", row["pick"]
  end

  def test_cypher_aliases_resolve_to_canonical_builtins
    r = @db.execute(<<~CYPHER)
      RETURN head([10, 20, 30])             AS head_alias,
             toLower('WORLD')               AS lower_alias,
             coalesce(null, 'fallback')     AS pick_alias,
             substring('hello-world', 6, 5) AS sub,
             toInteger('42')                AS as_int
    CYPHER
    row = r["rows"][0]
    assert_equal 10, row["head_alias"]
    assert_equal "world", row["lower_alias"]
    assert_equal "fallback", row["pick_alias"]
    assert_equal "world", row["sub"]
    assert_equal 42, row["as_int"]
  end

  def test_type_and_cast_namespaces
    # The Ruby binding marshals Ruby Integers as Cypher FLOATs, so the
    # inspected type is taken from a Cypher literal here rather than from
    # a parameter. The cast.* checks still exercise the runtime path.
    r = @db.execute(<<~CYPHER)
      RETURN type.of(7)                        AS kind,
             type.is(7, INTEGER)               AS is_int,
             cast.to('99', INTEGER)            AS cast_ok,
             cast.try('not-a-number', INTEGER) AS try_bad
    CYPHER
    row = r["rows"][0]
    assert_equal "INTEGER", row["kind"]
    assert_equal true, row["is_int"]
    assert_equal 99, row["cast_ok"]
    assert_nil row["try_bad"]
  end
end
