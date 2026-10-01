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
  # another thread is running a query. A ticker busy-increments a plain
  # Ruby counter; once it is running, a query that takes real time runs,
  # and the counter must have advanced across it. With the GVL held for
  # the whole call the ticker could not run in between. (An unfiltered
  # count is answered from the label count, too fast to tell anything.)
  def test_gvl_released_during_execute
    db = LoraRuby::Database.create

    counter = 0
    stop    = false
    ticker  = Thread.new do
      until stop
        counter += 1
      end
    end
    Thread.pass until counter > 0

    before = counter
    db.execute("UNWIND range(1, 2000000) AS i RETURN sum(i) AS s")
    during = counter - before
    stop = true
    ticker.join
    assert_operator during, :>, 0,
                    "ticker thread never made progress — GVL may not be released"
  end
end
