package lora

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestSnapshotRoundTrip(t *testing.T) {
	db, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer db.Close()

	if _, err := db.Execute("CREATE (:Person {name: 'Ada'}), (:Person {name: 'Grace'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if _, err := db.Execute("MATCH (a:Person {name:'Ada'}), (g:Person {name:'Grace'}) CREATE (a)-[:KNOWS]->(g)", nil); err != nil {
		t.Fatalf("edge: %v", err)
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "snap.bin")

	meta, err := db.SaveSnapshot(path)
	if err != nil {
		t.Fatalf("SaveSnapshot: %v", err)
	}
	if meta == nil {
		t.Fatal("expected non-nil SnapshotMeta")
	}
	if got := meta.NodeCount; got != 2 {
		t.Errorf("NodeCount = %d; want 2", got)
	}
	if got := meta.RelationshipCount; got != 1 {
		t.Errorf("RelationshipCount = %d; want 1", got)
	}
	if meta.FormatVersion == 0 {
		t.Errorf("FormatVersion should be non-zero, got 0")
	}
	if meta.WalLsn != nil {
		t.Errorf("pure snapshot should have WalLsn == nil, got %v", *meta.WalLsn)
	}
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("snapshot file missing: %v", err)
	}

	db2, err := New()
	if err != nil {
		t.Fatalf("New(2): %v", err)
	}
	defer db2.Close()

	meta2, err := db2.LoadSnapshot(path)
	if err != nil {
		t.Fatalf("LoadSnapshot: %v", err)
	}
	if meta2.NodeCount != 2 || meta2.RelationshipCount != 1 {
		t.Errorf("loaded counts wrong: %+v", meta2)
	}

	n, err := db2.NodeCount()
	if err != nil {
		t.Fatalf("NodeCount: %v", err)
	}
	if n != 2 {
		t.Errorf("graph NodeCount = %d; want 2", n)
	}
}

func TestSnapshotBytesBase64AndReaders(t *testing.T) {
	db, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer db.Close()

	if _, err := db.Execute("CREATE (:Snapshot {name: 'Ada'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}

	snapshotBytes, meta, err := db.SaveSnapshotBytes()
	if err != nil {
		t.Fatalf("SaveSnapshotBytes: %v", err)
	}
	if len(snapshotBytes) == 0 {
		t.Fatal("expected snapshot bytes")
	}
	if meta.NodeCount != 1 {
		t.Fatalf("SaveSnapshotBytes NodeCount = %d; want 1", meta.NodeCount)
	}

	var buf bytes.Buffer
	meta, err = db.SaveSnapshotTo(&buf)
	if err != nil {
		t.Fatalf("SaveSnapshotTo: %v", err)
	}
	if meta.NodeCount != 1 || buf.Len() == 0 {
		t.Fatalf("SaveSnapshotTo meta=%+v len=%d; want node and bytes", meta, buf.Len())
	}

	encoded, meta, err := db.SaveSnapshotBase64()
	if err != nil {
		t.Fatalf("SaveSnapshotBase64: %v", err)
	}
	if encoded == "" || meta.NodeCount != 1 {
		t.Fatalf("SaveSnapshotBase64 encoded=%q meta=%+v", encoded, meta)
	}

	for name, load := range map[string]func(*Database) (*SnapshotMeta, error){
		"bytes": func(target *Database) (*SnapshotMeta, error) {
			return target.LoadSnapshotBytes(snapshotBytes)
		},
		"reader": func(target *Database) (*SnapshotMeta, error) {
			return target.LoadSnapshotFrom(bytes.NewReader(buf.Bytes()))
		},
		"base64": func(target *Database) (*SnapshotMeta, error) {
			return target.LoadSnapshotBase64(encoded)
		},
	} {
		target, err := New()
		if err != nil {
			t.Fatalf("%s New: %v", name, err)
		}
		meta, err := load(target)
		if err != nil {
			t.Fatalf("%s load: %v", name, err)
		}
		if meta.NodeCount != 1 {
			t.Fatalf("%s NodeCount = %d; want 1", name, meta.NodeCount)
		}
		result, err := target.Execute("MATCH (n:Snapshot) RETURN n.name AS name", nil)
		if err != nil {
			t.Fatalf("%s Execute: %v", name, err)
		}
		if len(result.Rows) != 1 || result.Rows[0]["name"] != "Ada" {
			t.Fatalf("%s rows = %#v; want Ada", name, result.Rows)
		}
		target.Close()
	}
}

func TestWalBackedNewPersistsAcrossReopen(t *testing.T) {
	walDir := filepath.Join(t.TempDir(), "wal")

	db, err := New(walDir)
	if err != nil {
		t.Fatalf("New(wal): %v", err)
	}
	if _, err := db.Execute("CREATE (:Person {name: 'Ada'}), (:Person {name: 'Grace'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if _, err := db.Execute("MATCH (a:Person {name:'Ada'}), (g:Person {name:'Grace'}) CREATE (a)-[:KNOWS]->(g)", nil); err != nil {
		t.Fatalf("edge: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db2, err := New(walDir)
	if err != nil {
		t.Fatalf("New(reopen): %v", err)
	}
	defer db2.Close()

	n, err := db2.NodeCount()
	if err != nil {
		t.Fatalf("NodeCount: %v", err)
	}
	if n != 2 {
		t.Fatalf("NodeCount = %d; want 2", n)
	}
	r, err := db2.Execute("MATCH (p:Person) RETURN p.name AS name ORDER BY name", nil)
	if err != nil {
		t.Fatalf("Execute(reopen): %v", err)
	}
	if got, want := r.Rows, []map[string]any{{"name": "Ada"}, {"name": "Grace"}}; len(got) != len(want) {
		t.Fatalf("rows len = %d; want %d", len(got), len(want))
	} else {
		for i := range want {
			if got[i]["name"] != want[i]["name"] {
				t.Fatalf("rows[%d][name] = %v; want %v", i, got[i]["name"], want[i]["name"])
			}
		}
	}
}

func TestWalBackedNewAcceptsRelativePath(t *testing.T) {
	oldWd, err := os.Getwd()
	if err != nil {
		t.Fatalf("Getwd: %v", err)
	}
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("Chdir(tmp): %v", err)
	}
	defer os.Chdir(oldWd)

	db, err := New("relative-wal")
	if err != nil {
		t.Fatalf("New(relative): %v", err)
	}
	if _, err := db.Execute("CREATE (:Session {value: 'ok'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db2, err := New("relative-wal")
	if err != nil {
		t.Fatalf("New(relative reopen): %v", err)
	}
	defer db2.Close()

	r, err := db2.Execute("MATCH (s:Session) RETURN s.value AS value", nil)
	if err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if len(r.Rows) != 1 || r.Rows[0]["value"] != "ok" {
		t.Fatalf("rows = %#v; want value ok", r.Rows)
	}
}

func TestWalBackedNewInvalidPathSurfacesLoraError(t *testing.T) {
	notADir := filepath.Join(t.TempDir(), "wal-file")
	if err := os.WriteFile(notADir, []byte("not a directory"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}

	_, err := New(notADir)
	if err == nil {
		t.Fatal("expected an error")
	}
	var lerr *LoraError
	if !errors.As(err, &lerr) {
		t.Fatalf("expected *LoraError, got %T: %v", err, err)
	}
	if lerr.Code != CodeLoraError {
		t.Fatalf("error code = %s; want %s", lerr.Code, CodeLoraError)
	}
}

func TestLoadSnapshotMissingFileSurfacesLoraError(t *testing.T) {
	db, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer db.Close()

	_, err = db.LoadSnapshot(filepath.Join(t.TempDir(), "does-not-exist.bin"))
	if err == nil {
		t.Fatal("expected an error")
	}
	var lerr *LoraError
	if !errors.As(err, &lerr) {
		t.Fatalf("expected *LoraError, got %T: %v", err, err)
	}
}

func TestSnapshotMetaString(t *testing.T) {
	m := &SnapshotMeta{FormatVersion: 1, NodeCount: 3, RelationshipCount: 2}
	got := m.String()
	want := "SnapshotMeta{formatVersion=1, nodeCount=3, relationshipCount=2, walLsn=null}"
	if got != want {
		t.Errorf("String() = %q; want %q", got, want)
	}
	lsn := uint64(42)
	m.WalLsn = &lsn
	got = m.String()
	want = "SnapshotMeta{formatVersion=1, nodeCount=3, relationshipCount=2, walLsn=42}"
	if got != want {
		t.Errorf("String() = %q; want %q", got, want)
	}
}
