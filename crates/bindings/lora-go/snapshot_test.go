package lora

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
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
	databaseDir := filepath.Join(t.TempDir(), "data")

	db, err := New("app", Options{DatabaseDir: databaseDir})
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

	db2, err := New("app", Options{DatabaseDir: databaseDir})
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

func TestManagedWalSnapshotsRecoverSnapshotThenNewerWal(t *testing.T) {
	root := t.TempDir()
	walDir := filepath.Join(root, "wal")
	snapshotDir := filepath.Join(root, "snapshots")
	options := WalOptions{
		WalDir:               walDir,
		SnapshotDir:          snapshotDir,
		SnapshotEveryCommits: 2,
	}

	db, err := OpenWal(options)
	if err != nil {
		t.Fatalf("OpenWal(managed snapshots): %v", err)
	}
	if _, err := db.Execute("CREATE (:Managed {id: 1})", nil); err != nil {
		t.Fatalf("seed 1: %v", err)
	}
	if _, err := db.Execute("CREATE (:Managed {id: 2})", nil); err != nil {
		t.Fatalf("seed 2: %v", err)
	}
	if _, err := os.Stat(filepath.Join(snapshotDir, "CURRENT")); err != nil {
		t.Fatalf("CURRENT snapshot missing: %v", err)
	}
	if _, err := db.Execute("CREATE (:Managed {id: 3})", nil); err != nil {
		t.Fatalf("seed 3: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	reopened, err := OpenWal(options)
	if err != nil {
		t.Fatalf("OpenWal(reopen managed snapshots): %v", err)
	}
	defer reopened.Close()
	r, err := reopened.Execute("MATCH (n:Managed) RETURN n.id AS id ORDER BY id", nil)
	if err != nil {
		t.Fatalf("Execute(reopen): %v", err)
	}
	if got, want := len(r.Rows), 3; got != want {
		t.Fatalf("row count = %d; want %d (%#v)", got, want, r.Rows)
	}
	for i, row := range r.Rows {
		if got, want := row["id"], int64(i+1); got != want {
			t.Fatalf("row %d id = %#v; want %d", i, got, want)
		}
	}
}

func TestNewOptionsWithoutNameDoesNotEnablePersistence(t *testing.T) {
	_, err := New(Options{DatabaseDir: filepath.Join(t.TempDir(), "data")})
	if err == nil {
		t.Fatal("New(Options) succeeded; want database name error")
	}
	if !strings.Contains(err.Error(), "database name") {
		t.Fatalf("error = %v; want database name validation", err)
	}
}

func TestManagedSnapshotOptionsRequireSnapshotDir(t *testing.T) {
	_, err := OpenWal(WalOptions{
		WalDir:               filepath.Join(t.TempDir(), "wal"),
		SnapshotEveryCommits: 2,
	})
	if err == nil {
		t.Fatal("OpenWal(managed snapshot tuning without SnapshotDir) succeeded; want error")
	}
	if !strings.Contains(err.Error(), "SnapshotDir") {
		t.Fatalf("error = %v; want SnapshotDir validation", err)
	}
}

func TestWalBackedNewAcceptsRelativeDatabaseDir(t *testing.T) {
	oldWd, err := os.Getwd()
	if err != nil {
		t.Fatalf("Getwd: %v", err)
	}
	tmp := t.TempDir()
	if err := os.Chdir(tmp); err != nil {
		t.Fatalf("Chdir(tmp): %v", err)
	}
	defer os.Chdir(oldWd)

	db, err := New("app", Options{DatabaseDir: "relative-wal"})
	if err != nil {
		t.Fatalf("New(relative): %v", err)
	}
	if _, err := db.Execute("CREATE (:Session {value: 'ok'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db2, err := New("app", Options{DatabaseDir: "relative-wal"})
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

	_, err := New("app", Options{DatabaseDir: notADir})
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

func TestWalBackedNewInvalidNameSurfacesLoraError(t *testing.T) {
	_, err := New("../bad")
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

func TestEncryptedSnapshotBytesAndPath(t *testing.T) {
	db, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer db.Close()

	if _, err := db.Execute("CREATE (:Secret {name: 'Ada'})", nil); err != nil {
		t.Fatalf("seed: %v", err)
	}

	encryption := &SnapshotEncryption{
		Type:     "password",
		KeyID:    "go-test",
		Password: "open sesame",
		Params: &SnapshotPasswordParams{
			MemoryCostKib: 512,
			TimeCost:      1,
			Parallelism:   1,
		},
	}
	saveOptions := &SnapshotOptions{
		Compression: &SnapshotCompression{Format: "gzip", Level: 1},
		Encryption:  encryption,
	}
	loadOptions := &SnapshotLoadOptions{Credentials: encryption}

	snapshotBytes, meta, err := db.SaveSnapshotBytesWithOptions(saveOptions)
	if err != nil {
		t.Fatalf("SaveSnapshotBytesWithOptions: %v", err)
	}
	if len(snapshotBytes) == 0 || meta.NodeCount != 1 {
		t.Fatalf("encrypted bytes meta=%+v len=%d; want one node and bytes", meta, len(snapshotBytes))
	}

	target, err := New()
	if err != nil {
		t.Fatalf("New target: %v", err)
	}
	defer target.Close()
	if _, err := target.LoadSnapshotBytes(snapshotBytes); err == nil {
		t.Fatal("LoadSnapshotBytes without credentials succeeded; want error")
	}
	meta, err = target.LoadSnapshotBytesWithOptions(snapshotBytes, loadOptions)
	if err != nil {
		t.Fatalf("LoadSnapshotBytesWithOptions: %v", err)
	}
	if meta.NodeCount != 1 {
		t.Fatalf("LoadSnapshotBytesWithOptions NodeCount = %d; want 1", meta.NodeCount)
	}

	path := filepath.Join(t.TempDir(), "secret.lsnap")
	meta, err = db.SaveSnapshotWithOptions(path, saveOptions)
	if err != nil {
		t.Fatalf("SaveSnapshotWithOptions: %v", err)
	}
	if meta.NodeCount != 1 {
		t.Fatalf("SaveSnapshotWithOptions NodeCount = %d; want 1", meta.NodeCount)
	}

	targetFromPath, err := New()
	if err != nil {
		t.Fatalf("New targetFromPath: %v", err)
	}
	defer targetFromPath.Close()
	if _, err := targetFromPath.LoadSnapshot(path); err == nil {
		t.Fatal("LoadSnapshot without credentials succeeded; want error")
	}
	meta, err = targetFromPath.LoadSnapshotWithOptions(path, &SnapshotLoadOptions{Encryption: encryption})
	if err != nil {
		t.Fatalf("LoadSnapshotWithOptions: %v", err)
	}
	if meta.NodeCount != 1 {
		t.Fatalf("LoadSnapshotWithOptions NodeCount = %d; want 1", meta.NodeCount)
	}
	result, err := targetFromPath.Execute("MATCH (n:Secret) RETURN n.name AS name", nil)
	if err != nil {
		t.Fatalf("Execute: %v", err)
	}
	if len(result.Rows) != 1 || result.Rows[0]["name"] != "Ada" {
		t.Fatalf("rows = %#v; want Ada", result.Rows)
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
