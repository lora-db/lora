use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lora_database::{Database, InMemoryGraph, SnapshotAdmin, WalAdmin, WalConfig};
use lora_server::config::{self, ConfigOutcome, ServerConfig, DEFAULT_WAL_SEGMENT_TARGET_BYTES};
use lora_server::{serve_with_admin, AdminConfig, SnapshotAdminConfig};

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

    let runtime = match tokio::runtime::Builder::new_current_thread()
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
    let admin_config = build_admin_config(&cfg, &db);

    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    println!("Lora server running at http://{local}");
    serve_with_admin(listener, db, admin_config).await
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
    println!(
        "Recovered from {} + WAL {} (sync-mode = {:?})",
        snapshot.display(),
        dir.display(),
        cfg.wal_sync_mode
    );
    Ok(db)
}

fn wal_config(cfg: &ServerConfig, dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: cfg.wal_sync_mode,
        segment_target_bytes: DEFAULT_WAL_SEGMENT_TARGET_BYTES,
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
) -> Option<AdminConfig> {
    let snapshot_admin = snapshot_admin_config(cfg, db);
    let wal_admin = wal_admin_config(cfg, db);

    if snapshot_admin.is_some() || wal_admin.is_some() {
        announce_admin_routes(snapshot_admin.as_ref(), wal_admin.as_ref());
        Some(AdminConfig {
            snapshot: snapshot_admin,
            wal: wal_admin,
        })
    } else {
        None
    }
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
            "Admin routes enabled: /admin/wal/{{status,truncate}} + /admin/checkpoint (path in body required)",
        ),
        (None, None) => {}
    }
}
