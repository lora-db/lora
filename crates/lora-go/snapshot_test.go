package lora

import (
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
