package lora_test

import (
	"fmt"
	"runtime"
	"sync"
	"testing"
	"time"

	lora "github.com/lora-db/lora/crates/bindings/lora-go"
)

// concurrencyDeadline bounds every scenario here, so a deadlock fails the
// test instead of hanging until `go test -timeout`.
const concurrencyDeadline = 30 * time.Second

func withinDeadline(t *testing.T, name string, work func()) {
	t.Helper()
	done := make(chan struct{})
	go func() {
		defer close(done)
		work()
	}()
	select {
	case <-done:
	case <-time.After(concurrencyDeadline):
		buf := make([]byte, 1<<20)
		n := runtime.Stack(buf, true)
		t.Fatalf("%s: hung for %s\n%s", name, concurrencyDeadline, buf[:n])
	}
}

// drain pulls a stream to its end, yielding between rows so the goroutine
// hops OS threads while the stream (and, for a mutating one, the writer
// lock) stays open.
func drain(it *lora.RowIterator) (int, error) {
	n := 0
	for it.Next() {
		n++
		runtime.Gosched()
	}
	return n, it.Err()
}

// 50 goroutines on GOMAXPROCS=1 write at once: auto-commit writes, batched
// transactions, and mutating streams that yield between rows while they
// hold the writer lock. A goroutine blocked in a cgo call waiting for the
// lock gives up its P, so the goroutine holding the lock always gets to
// run its next call.
func TestConcurrentWritersOnOneProc(t *testing.T) {
	defer runtime.GOMAXPROCS(runtime.GOMAXPROCS(1))
	db := newDB(t)

	const writers = 50
	errs := make(chan error, writers)
	withinDeadline(t, "writers", func() {
		var wg sync.WaitGroup
		for i := 0; i < writers; i++ {
			wg.Add(1)
			go func(i int) {
				defer wg.Done()
				var err error
				switch i % 3 {
				case 0:
					_, err = db.Execute("CREATE (:W {i: $i})", lora.Params{"i": i})
				case 1:
					_, err = db.Transaction([]lora.TransactionStatement{
						{Query: "CREATE (:W {i: $i})", Params: lora.Params{"i": i}},
						{Query: "MATCH (n:W) RETURN count(n) AS c"},
					}, lora.TransactionReadWrite)
				default:
					var it *lora.RowIterator
					it, err = db.Stream("UNWIND [1, 2] AS k CREATE (:W {i: $i, k: k}) RETURN k", lora.Params{"i": i})
					if err == nil {
						var n int
						n, err = drain(it)
						if err == nil && n != 2 {
							err = fmt.Errorf("stream %d: %d rows", i, n)
						}
					}
				}
				if err != nil {
					errs <- err
				}
			}(i)
		}
		wg.Wait()
	})
	close(errs)
	for err := range errs {
		t.Error(err)
	}
	n, err := db.NodeCount()
	if err != nil {
		t.Fatal(err)
	}
	// 17 auto-commit + 17 transactions + 16 streams x 2 rows.
	if n != 17+17+16*2 {
		t.Fatalf("node count = %d", n)
	}
}

// A mutating stream opened in one goroutine, pulled in another and closed
// in a third (as a finalizer would) commits or rolls back and frees the
// writer lock, whatever OS threads those calls ran on.
func TestMutatingStreamAcrossGoroutines(t *testing.T) {
	defer runtime.GOMAXPROCS(runtime.GOMAXPROCS(1))
	db := newDB(t)
	withinDeadline(t, "stream across goroutines", func() {
		run := func(f func()) {
			done := make(chan struct{})
			go func() {
				defer close(done)
				runtime.LockOSThread() // a fresh OS thread for each step
				f()
			}()
			<-done
		}

		var it *lora.RowIterator
		run(func() {
			var err error
			it, err = db.Stream("UNWIND range(1, 3) AS i CREATE (:Kept {i: i}) RETURN i", nil)
			if err != nil {
				t.Error(err)
			}
		})
		run(func() {
			if _, err := drain(it); err != nil {
				t.Error(err)
			}
		})
		run(func() { _ = it.Close() })

		run(func() {
			var err error
			it, err = db.Stream("UNWIND range(1, 3) AS i CREATE (:Dropped {i: i}) RETURN i", nil)
			if err != nil {
				t.Error(err)
			}
		})
		run(func() { it.Next() })
		run(func() { _ = it.Close() })

		// The writer lock is free again.
		var wg sync.WaitGroup
		for i := 0; i < 8; i++ {
			wg.Add(1)
			go func() {
				defer wg.Done()
				if _, err := db.Execute("CREATE (:After)", nil); err != nil {
					t.Error(err)
				}
			}()
		}
		wg.Wait()
	})
	n, err := db.NodeCount()
	if err != nil {
		t.Fatal(err)
	}
	if n != 3+8 {
		t.Fatalf("node count = %d, want 11 (3 committed, 3 rolled back, 8 after)", n)
	}
}

// Closing the database while a mutating stream is still open is safe: the
// stream keeps the engine alive until it is closed.
func TestStreamOutlivesClosedDatabase(t *testing.T) {
	withinDeadline(t, "stream after close", func() {
		for _, early := range []bool{false, true} {
			db, err := lora.New()
			if err != nil {
				t.Fatal(err)
			}
			it, err := db.Stream("UNWIND range(1, 3) AS i CREATE (:S {i: i}) RETURN i", nil)
			if err != nil {
				t.Fatal(err)
			}
			if err := db.Close(); err != nil {
				t.Fatal(err)
			}
			if early {
				it.Next()
			} else if _, err := drain(it); err != nil {
				t.Fatal(err)
			}
			_ = it.Close()
		}
	})
}
