# frozen_string_literal: true

require_relative "test_helper"
require "rbconfig"

# Writers waiting for the engine's writer lock must never hold the GVL:
# the writer that holds the lock runs GVL-free and never needs the GVL to
# release it, so a waiter that kept the GVL would only freeze every other
# Ruby thread (and would deadlock the process if a lock were ever held
# across Ruby calls). Each scenario runs in a child process with a hard
# timeout so a regression fails the test instead of hanging the runner.
class TestWriterLock < Minitest::Test
  LIB = File.expand_path("../lib", __dir__)
  TIMEOUT = 30

  def run_child(script)
    reader, writer = IO.pipe
    pid = Process.spawn(RbConfig.ruby, "-I", LIB, "-r", "lora_ruby", "-e", script,
                        out: writer, err: writer)
    writer.close
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + TIMEOUT
    status = nil
    loop do
      _, status = Process.wait2(pid, Process::WNOHANG)
      break if status
      if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
        Process.kill(:KILL, pid)
        Process.wait(pid)
        flunk "child hung for #{TIMEOUT}s:\n#{reader.read}"
      end
      sleep 0.05
    end
    output = reader.read
    assert status.success?, "child failed (#{status.inspect}):\n#{output}"
    output
  ensure
    reader&.close
  end

  # 50 writer threads (more than any pool or core count), each issuing
  # several writes and yielding between them, mixed with readers and
  # clear calls.
  def test_many_writer_threads_mixed_with_reads_and_clear
    out = run_child(<<~RUBY)
      db = LoraRuby::Database.create
      threads = Array.new(50) do |t|
        Thread.new do
          5.times do |i|
            db.execute("CREATE (:N {t: $t, i: $i})", { t: t, i: i })
            Thread.pass
            sleep 0.001
          end
        end
      end
      threads += Array.new(5) do
        Thread.new { 10.times { db.execute("MATCH (n:N) RETURN count(n) AS c"); db.node_count } }
      end
      threads.each(&:join)
      raise "expected 250 nodes, got \#{db.node_count}" unless db.node_count == 250
      clears = Array.new(10) { Thread.new { db.execute("CREATE (:M)"); db.clear } }
      clears.each(&:join)
      puts "ok"
    RUBY
    assert_includes out, "ok"
  end

  # clear waits for the writer lock; while it waits, other Ruby threads
  # must keep running (the GVL is released for the wait).
  def test_clear_waits_for_the_writer_lock_without_the_gvl
    out = run_child(<<~RUBY)
      db = LoraRuby::Database.create
      ticks = 0
      stop = false
      ticker = Thread.new { until stop; ticks += 1; Thread.pass; end }
      writer = Thread.new { db.execute("UNWIND range(1, 400000) AS i CREATE (:X {i: i})") }
      sleep 0.05
      before = ticks
      db.clear
      during = ticks - before
      writer.join
      stop = true
      ticker.join
      puts "ticks=\#{during}"
    RUBY
    ticks = out[/ticks=(\d+)/, 1].to_i
    assert_operator ticks, :>, 0, "Ruby threads froze while clear waited for the writer lock"
  end

  # The process exits promptly while many writers are running or waiting
  # for the writer lock.
  def test_exit_while_writers_wait
    out = run_child(<<~RUBY)
      db = LoraRuby::Database.create
      30.times do
        Thread.new { loop { db.execute("UNWIND range(1, 20000) AS i CREATE (:X {i: i})") } }
      end
      sleep 0.2
      puts "exiting"
      exit 0
    RUBY
    assert_includes out, "exiting"
  end

  # Thread#kill on threads waiting for the writer lock takes effect once
  # their call returns (no unblock function), and never wedges the rest.
  def test_kill_threads_waiting_for_the_writer_lock
    out = run_child(<<~RUBY)
      db = LoraRuby::Database.create
      writer = Thread.new { db.execute("UNWIND range(1, 400000) AS i CREATE (:X {i: i})") }
      sleep 0.05
      waiters = Array.new(10) { Thread.new { db.execute("CREATE (:Y)") } }
      sleep 0.05
      waiters.each(&:kill)
      waiters.each(&:join)
      writer.join
      db.execute("CREATE (:Z)")
      puts "ok"
    RUBY
    assert_includes out, "ok"
  end
end
