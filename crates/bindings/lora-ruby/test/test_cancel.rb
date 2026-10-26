# frozen_string_literal: true

require_relative "test_helper"
require "rbconfig"
require "timeout"
require "tmpdir"

# Long queries can be bounded (`timeout_ms:`), and a query with a timeout
# can be interrupted (any Ruby interrupt of the calling thread cancels it).
# Both leave the database usable: the query rolls back and releases its
# locks.
class TestCancel < Minitest::Test
  # A three-way cartesian product over 2000 nodes: minutes of work.
  SLOW = "MATCH (a:N), (b:N), (c:N) RETURN count(*) AS c"

  def setup
    @db = LoraRuby::Database.create
    @db.execute("UNWIND range(1, 2000) AS i CREATE (:N {i: i})")
  end

  def test_timeout_ms_stops_a_slow_query
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    err = assert_raises(LoraRuby::QueryError) { @db.execute(SLOW, nil, timeout_ms: 200) }
    assert_match(/LORA_TIMEOUT/, err.message)
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 20

    err = assert_raises(LoraRuby::QueryError) { @db.profile(SLOW, timeout_ms: 200) }
    assert_match(/LORA_TIMEOUT/, err.message)

    # A fast query is unaffected by a timeout.
    rows = @db.execute("MATCH (n:N) RETURN count(n) AS c", nil, timeout_ms: 10_000)["rows"]
    assert_equal 2000, rows.first["c"]
  end

  def test_rejects_a_non_positive_timeout
    assert_raises(LoraRuby::InvalidParamsError) { @db.execute("RETURN 1", timeout_ms: 0) }
    assert_raises(LoraRuby::InvalidParamsError) { @db.execute("RETURN 1", timeout_ms: -5) }
  end

  def test_keyword_params_still_bind
    row = @db.execute("RETURN $a AS a, $b AS b", a: 1, b: "x")["rows"].first
    assert_equal({ "a" => 1, "b" => "x" }, row)
    row = @db.execute("RETURN $a AS a", { a: 2 }, timeout_ms: 5_000)["rows"].first
    assert_equal({ "a" => 2 }, row)
    assert_raises(ArgumentError) { @db.execute("RETURN $a AS a", { a: 2 }, a: 3) }
  end

  def test_timeout_interrupt_cancels_the_query
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    assert_raises(Timeout::Error) do
      Timeout.timeout(0.3) { @db.execute(SLOW, timeout_ms: 600_000) }
    end
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 20
  end

  def test_interrupted_write_rolls_back_and_frees_the_writer_lock
    writer = Thread.new do
      @db.execute("MATCH (a:N), (b:N), (c:N) CREATE (:Junk)", timeout_ms: 600_000)
    rescue StandardError, Timeout::Error
      nil
    end
    sleep 0.3
    writer.raise(Timeout::Error, "stop")
    assert writer.join(20), "interrupted writer did not stop"

    # The lock is free and nothing from the interrupted write committed.
    Timeout.timeout(20) { @db.execute("CREATE (:After)") }
    assert_equal 0, @db.execute("MATCH (j:Junk) RETURN count(j) AS c")["rows"].first["c"]
  end

  # The GC frees an unreachable WAL database off the GVL; reopening the
  # directory waits for that to finish, and so does process exit.
  def test_gc_dropped_wal_database_closes_cleanly
    Dir.mktmpdir("lora-ruby-gc-") do |dir|
      wal = File.join(dir, "wal")
      script = <<~RUBY
        $LOAD_PATH.unshift #{File.expand_path("../lib", __dir__).inspect}
        require "lora_ruby"
        db = LoraRuby::Database.open_wal(#{wal.inspect})
        db.execute("CREATE (:Kept {v: 1})")
        db = nil
        GC.start(full_mark: true, immediate_sweep: true)
        again = LoraRuby::Database.open_wal(#{wal.inspect})
        again.execute("CREATE (:Kept {v: 2})")
        again = nil
        GC.start(full_mark: true, immediate_sweep: true)
      RUBY
      assert system(RbConfig.ruby, "-e", script), "child process failed"

      db = LoraRuby::Database.open_wal(wal)
      values = db.execute("MATCH (k:Kept) RETURN k.v AS v ORDER BY v")["rows"].map { |r| r["v"] }
      assert_equal [1, 2], values
      db.close
    end
  end
end
