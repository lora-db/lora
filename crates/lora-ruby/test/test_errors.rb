# frozen_string_literal: true

require_relative "test_helper"

class TestErrors < Minitest::Test
  def setup
    @db = LoraRuby::Database.create
  end

  def test_parse_error_raises_query_error
    assert_raises(LoraRuby::QueryError) do
      @db.execute("THIS IS NOT CYPHER")
    end
  end

  def test_query_error_is_subclass_of_error
    assert_operator LoraRuby::QueryError, :<, LoraRuby::Error
    assert_operator LoraRuby::InvalidParamsError, :<, LoraRuby::Error
  end

  def test_error_is_standard_error_subclass
    # Lets callers `rescue => e` if they want a catch-all, matching
    # Ruby idiom.
    assert_operator LoraRuby::Error, :<, StandardError
  end

  def test_invalid_params_is_distinct_from_query_error
    # Parse errors → QueryError. Param-parsing errors →
    # InvalidParamsError. Callers should be able to branch on the
    # distinction.
    refute_operator LoraRuby::InvalidParamsError, :<=, LoraRuby::QueryError
    refute_operator LoraRuby::QueryError, :<=, LoraRuby::InvalidParamsError
  end
end
