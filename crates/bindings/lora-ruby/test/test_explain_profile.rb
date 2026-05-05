# frozen_string_literal: true

require_relative "test_helper"

class TestExplainProfile < Minitest::Test
  def setup
    @db = LoraRuby::Database.create
  end

  def test_explain_does_not_execute_mutating_query
    plan = @db.explain("CREATE (:Foo {n: 1})")
    assert_equal "mutating", plan["shape"]
    assert_equal 0, @db.node_count
  end

  def test_explain_returns_plan_tree
    @db.execute("CREATE (:Person {name: 'Alice'})")
    plan = @db.explain("MATCH (p:Person) RETURN p")
    assert_equal "readOnly", plan["shape"]
    assert_equal ["p"], plan["result_columns"]
    assert plan["tree"]["operator"]
  end

  def test_explain_with_params_forwarded
    @db.execute("CREATE (:Person {name: 'Alice'})")
    plan = @db.explain(
      "MATCH (p:Person) WHERE p.name = $name RETURN p",
      { "name" => "Alice" }
    )
    assert_equal "readOnly", plan["shape"]
  end

  def test_profile_executes_mutating_query
    profile = @db.profile("CREATE (:Foo {n: 1}) RETURN 1 AS one")
    assert_equal true, profile["metrics"]["mutated"]
    assert_equal 1, profile["metrics"]["total_rows"]
    assert_equal 1, @db.node_count
  end

  def test_profile_reports_per_operator_timing
    %w[Alice Bob Carol Dave].each do |name|
      @db.execute("CREATE (:Person {name: '#{name}'})")
    end
    profile = @db.profile(
      "MATCH (p:Person) WHERE p.name <> 'Bob' RETURN p.name AS name"
    )
    assert_equal 3, profile["metrics"]["total_rows"]
    refute_empty profile["metrics"]["per_operator"]
    profile["metrics"]["per_operator"].each_value do |op|
      assert op["next_calls"] > 0
    end
  end

  def test_profile_with_params_forwarded
    @db.execute("CREATE (:Person {name: 'Alice'})")
    @db.execute("CREATE (:Person {name: 'Bob'})")
    profile = @db.profile(
      "MATCH (p:Person) WHERE p.name = $name RETURN p",
      { "name" => "Alice" }
    )
    assert_equal 1, profile["metrics"]["total_rows"]
  end
end
