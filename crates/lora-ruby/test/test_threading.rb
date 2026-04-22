# frozen_string_literal: true

require_relative "test_helper"

class TestThreading < Minitest::Test
  # Concurrent queries against the same Database serialise on a Mutex
  # but MUST NOT deadlock and MUST produce the right final counts.
  def test_concurrent_inserts_converge_to_correct_count
    db     = LoraRuby::Database.create
    per    = 25
    nthr   = 4
    threads = Array.new(nthr) do |t|
      Thread.new do
        per.times do |i|
          db.execute("CREATE (:N {t: $t, i: $i})", { t: t, i: i })
        end
      end
    end
    threads.each(&:join)
    assert_equal per * nthr, db.node_count
  end

  # Different Database instances share no state — background inserts
  # on `bg` must not affect `fg` and vice versa.
  def test_separate_databases_are_isolated
    fg = LoraRuby::Database.create
    bg = LoraRuby::Database.create
    t = Thread.new do
      200.times { bg.execute("CREATE (:X)") }
    end
    100.times { fg.execute("CREATE (:Y)") }
    t.join
    assert_equal 100, fg.node_count
    assert_equal 200, bg.node_count
  end

  # GVL release must let an unrelated Ruby thread make progress while
  # another thread is running a query. We busy-increment a plain Ruby
  # counter during a ~2k-node MATCH and assert the counter ticked.
  # Passes with GVL release; would wedge at 0 on a naive
  # GVL-held implementation.
  def test_gvl_released_during_execute
    db = LoraRuby::Database.create
    2_000.times { db.execute("CREATE (:N)") }

    counter = 0
    stop    = false
    ticker  = Thread.new do
      until stop
        counter += 1
      end
    end
    # Run a non-trivial query under the GVL-released region. Even on
    # a very fast machine the counter should tick a handful of times.
    db.execute("MATCH (n:N) RETURN count(n) AS c")
    stop = true
    ticker.join
    assert_operator counter, :>, 0,
                    "ticker thread never made progress — GVL may not be released"
  end
end
