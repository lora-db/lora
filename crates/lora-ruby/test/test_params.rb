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
    r = @db.execute("RETURN vector([1,2,3], 3, INTEGER) AS v")
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
      "RETURN vector.similarity.cosine(vector([1.0, 0.0, 0.0], 3, FLOAT32), $q) AS s",
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
end
