use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lora_database::{Database, InMemoryGraph, SnapshotAdmin, WalAdmin, WalConfig};
use lora_server::config::{self, ConfigOutcome, ServerConfig, DEFAULT_WAL_SEGMENT_TARGET_BYTES};
use lora_server::{serve_with, AdminConfig, QueryLimits, SnapshotAdminConfig};

fn main() -> ExitCode {
    let cfg = match config::resolve_from_process() {
        Ok(ConfigOutcome::Run(cfg)) => cfg,
        Ok(ConfigOutcome::Help(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Ok(ConfigOutcome::Version(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("lora-server: {err}");
            eprintln!("Run `lora-server --help` for usage.");
            return ExitCode::from(2);
        }
    };

    // Engine calls run on the blocking pool (see `QueryLimits`); the worker
    // threads only parse requests and serialise responses, so a large result
    // being encoded never holds up other connections.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("lora-server: failed to start tokio runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("lora-server: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cfg: ServerConfig) -> anyhow::Result<()> {
    let db = open_database(&cfg)?;
    let admin_config = build_admin_config(&cfg, &db)?;
    let limits = QueryLimits {
        timeout: (cfg.query_timeout_ms > 0)
            .then(|| std::time::Duration::from_millis(cfg.query_timeout_ms)),
        max_concurrent: (cfg.max_concurrent_queries > 0).then_some(cfg.max_concurrent_queries),
        max_body_bytes: cfg.max_body_bytes,
        max_result_rows: (cfg.max_result_rows > 0).then_some(cfg.max_result_rows),
        max_response_bytes: (cfg.max_response_bytes > 0).then_some(cfg.max_response_bytes),
        header_read_timeout: (cfg.header_read_timeout_ms > 0)
            .then(|| std::time::Duration::from_millis(cfg.header_read_timeout_ms)),
    };

    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    println!("Lora server running at http://{local}");
    serve_with(listener, db, limits, admin_config).await
}

// Four boot paths, picked by the (--wal-dir, --restore-from) cross product:
//
//   wal_dir = None, restore_from = None           -> fresh in-memory DB.
//   wal_dir = None, restore_from = Some(snap)     -> load snap, no WAL.
//   wal_dir = Some(dir), restore_from = None      -> fresh graph + WAL.
//   wal_dir = Some(dir), restore_from = Some(snap)-> Database::recover:
//                                                   load snap, replay WAL
//                                                   past its fence.
fn open_database(cfg: &ServerConfig) -> anyhow::Result<Arc<Database<InMemoryGraph>>> {
    Ok(match (&cfg.wal_dir, &cfg.restore_from) {
        (None, None) => Arc::new(Database::in_memory()),
        (None, Some(path)) => Arc::new(open_snapshot_database(path)?),
        (Some(dir), None) => Arc::new(open_wal_database(cfg, dir)?),
        (Some(dir), Some(snap)) => Arc::new(recover_wal_database(cfg, dir, snap)?),
    })
}

fn open_snapshot_database(path: &Path) -> anyhow::Result<Database<InMemoryGraph>> {
    let db = Database::in_memory();
    restore_snapshot_if_present(&db, path)?;
    Ok(db)
}

fn open_wal_database(cfg: &ServerConfig, dir: &Path) -> anyhow::Result<Database<InMemoryGraph>> {
    let db = Database::open_with_wal(wal_config(cfg, dir))?;
    report_corrupt_cut(&db);
    println!(
        "WAL attached at {} (sync-mode = {:?})",
        dir.display(),
        cfg.wal_sync_mode
    );
    Ok(db)
}

fn recover_wal_database(
    cfg: &ServerConfig,
    dir: &Path,
    snapshot: &Path,
) -> anyhow::Result<Database<InMemoryGraph>> {
    let db = Database::recover(snapshot, wal_config(cfg, dir))?;
    report_corrupt_cut(&db);
    println!(
        "Recovered from {} + WAL {} (sync-mode = {:?})",
        snapshot.display(),
        dir.display(),
        cfg.wal_sync_mode
    );
    Ok(db)
}

/// Say on stderr where `--wal-truncate-corrupt-tail` cut the WAL, if it
/// had to: the server has no log subscriber, so the WAL's own warning is
/// not printed.
fn report_corrupt_cut(db: &Database<InMemoryGraph>) {
    let Some(cut) = db.wal().and_then(|recorder| recorder.wal().corrupt_cut()) else {
        return;
    };
    eprintln!(
        "WARNING: the WAL was corrupt: {} at byte {}: {}",
        cut.segment_path.display(),
        cut.offset,
        cut.reason
    );
    eprintln!(
        "WARNING: --wal-truncate-corrupt-tail cut the log there; writes after that point were not recovered."
    );
    if let Some(kept) = &cut.kept_tail {
        eprintln!("WARNING: the cut bytes were kept in {}", kept.display());
    }
    for (from, to) in &cut.moved_segments {
        eprintln!(
            "WARNING: later segment {} was moved to {}",
            from.display(),
            to.display()
        );
    }
}

fn wal_config(cfg: &ServerConfig, dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: cfg.wal_sync_mode,
        segment_target_bytes: DEFAULT_WAL_SEGMENT_TARGET_BYTES,
        truncate_corrupt_tail: cfg.wal_truncate_corrupt_tail,
    }
}

fn restore_snapshot_if_present(db: &Database<InMemoryGraph>, path: &Path) -> anyhow::Result<()> {
    match std::fs::metadata(path) {
        Ok(_) => {
            let meta = db.load_snapshot_from(path)?;
            println!(
                "Restored snapshot from {} ({} nodes, {} relationships)",
                path.display(),
                meta.node_count,
                meta.relationship_count
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "No snapshot at {} — starting with an empty graph",
                path.display()
            );
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

// Opt-in admin surface. Snapshot routes mount when --snapshot-path is set; WAL
// routes mount independently when --wal-dir is set, so a WAL-only deployment
// still gets /admin/wal/{status,truncate} and /admin/checkpoint.
fn build_admin_config(
    cfg: &ServerConfig,
    db: &Arc<Database<InMemoryGraph>>,
) -> anyhow::Result<Option<AdminConfig>> {
    let snapshot_admin = snapshot_admin_config(cfg, db);
    let wal_admin = wal_admin_config(cfg, db);

    if snapshot_admin.is_none() && wal_admin.is_none() {
        return Ok(None);
    }
    // Canonicalise once so request paths are compared against the real
    // directory, not whatever relative spelling the operator typed.
    let path_root = match &cfg.admin_path_root {
        Some(root) => Some(
            std::fs::canonicalize(root)
                .map_err(|e| anyhow::anyhow!("--admin-path-root {}: {e}", root.display()))?,
        ),
        None => None,
    };
    announce_admin_routes(snapshot_admin.as_ref(), wal_admin.as_ref());
    if let Some(root) = &path_root {
        println!("Admin `path` overrides allowed inside {}", root.display());
    }
    println!(
        "WARNING: the admin routes have no authentication. Do not expose them on an untrusted network."
    );
    Ok(Some(AdminConfig {
        snapshot: snapshot_admin,
        wal: wal_admin,
        path_root,
    }))
}

fn snapshot_admin_config(
    cfg: &ServerConfig,
    db: &Arc<Database<InMemoryGraph>>,
) -> Option<SnapshotAdminConfig> {
    cfg.snapshot_path.as_ref().map(|path| SnapshotAdminConfig {
        path: path.clone(),
        admin: Arc::clone(db) as Arc<dyn SnapshotAdmin>,
    })
}

fn wal_admin_config(
    cfg: &ServerConfig,
    db: &Arc<Database<InMemoryGraph>>,
) -> Option<Arc<dyn WalAdmin>> {
    cfg.wal_dir
        .as_ref()
        .map(|_| Arc::clone(db) as Arc<dyn WalAdmin>)
}

fn announce_admin_routes(
    snapshot_admin: Option<&SnapshotAdminConfig>,
    wal_admin: Option<&Arc<dyn WalAdmin>>,
) {
    match (snapshot_admin, wal_admin) {
        (Some(s), Some(_)) => println!(
            "Admin routes enabled: snapshot {{save,load}} + checkpoint / wal/{{status,truncate}} → {}",
            s.path.display()
        ),
        (Some(s), None) => println!(
            "Admin routes enabled: POST /admin/snapshot/{{save,load}} → {}",
            s.path.display()
        ),
        (None, Some(_)) => println!(
            "Admin routes enabled: /admin/wal/{{status,truncate}} + /admin/checkpoint (path in body, needs --admin-path-root)",
        ),
        (None, None) => {}
    }
}
