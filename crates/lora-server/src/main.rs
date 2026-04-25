use std::process::ExitCode;
use std::sync::Arc;

use lora_database::{Database, SnapshotAdmin, WalAdmin, WalConfig};
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
    // Three boot paths, picked by the (--wal-dir, --restore-from) cross
    // product:
    //
    //   wal_dir = None, restore_from = None           → fresh in-memory DB.
    //   wal_dir = None, restore_from = Some(snap)     → load snap, no WAL.
    //   wal_dir = Some(dir), restore_from = None      → fresh graph + WAL.
    //   wal_dir = Some(dir), restore_from = Some(snap)→ Database::recover:
    //                                                   load snap, replay
    //                                                   WAL past its fence.
    //
    // The "missing snapshot file is fine" semantics carry over: it lets
    // operators pass the same path on every boot regardless of whether
    // the file exists yet.
    let db: Arc<Database<lora_database::InMemoryGraph>> = match (&cfg.wal_dir, &cfg.restore_from) {
        (None, None) => Arc::new(Database::in_memory()),
        (None, Some(path)) => {
            let db = Database::in_memory();
            match std::fs::metadata(path) {
                Ok(_) => {
                    let meta = db.load_snapshot_from(path)?;
                    println!(
                        "Restored snapshot from {} ({} nodes, {} relationships)",
                        path.display(),
                        meta.node_count,
                        meta.relationship_count
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!(
                        "No snapshot at {} — starting with an empty graph",
                        path.display()
                    );
                }
                Err(e) => return Err(e.into()),
            }
            Arc::new(db)
        }
        (Some(dir), None) => {
            let wal_config = WalConfig::Enabled {
                dir: dir.clone(),
                sync_mode: cfg.wal_sync_mode,
                segment_target_bytes: DEFAULT_WAL_SEGMENT_TARGET_BYTES,
            };
            let db = Database::open_with_wal(wal_config)?;
            println!(
                "WAL attached at {} (sync-mode = {:?})",
                dir.display(),
                cfg.wal_sync_mode
            );
            Arc::new(db)
        }
        (Some(dir), Some(snap)) => {
            let wal_config = WalConfig::Enabled {
                dir: dir.clone(),
                sync_mode: cfg.wal_sync_mode,
                segment_target_bytes: DEFAULT_WAL_SEGMENT_TARGET_BYTES,
            };
            let db = Database::recover(snap, wal_config)?;
            println!(
                "Recovered from {} + WAL {} (sync-mode = {:?})",
                snap.display(),
                dir.display(),
                cfg.wal_sync_mode
            );
            Arc::new(db)
        }
    };

    // Opt-in admin surface. Snapshot routes mount when --snapshot-path
    // is set; WAL routes mount independently when --wal-dir is set, so
    // a WAL-only deployment still gets /admin/wal/{status,truncate} and
    // /admin/checkpoint (the latter requires `path` in the body without
    // a configured --snapshot-path). When neither is set, no /admin
    // routes exist.
    let snapshot_admin =
        cfg.snapshot_path
            .as_ref()
            .map(|path| SnapshotAdminConfig {
                path: path.clone(),
                admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
            });
    let wal_admin: Option<Arc<dyn WalAdmin>> = if cfg.wal_dir.is_some() {
        Some(Arc::clone(&db) as Arc<dyn WalAdmin>)
    } else {
        None
    };

    let admin_config = if snapshot_admin.is_some() || wal_admin.is_some() {
        match (snapshot_admin.as_ref(), wal_admin.as_ref()) {
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
            (None, None) => unreachable!(),
        }
        Some(AdminConfig {
            snapshot: snapshot_admin,
            wal: wal_admin,
        })
    } else {
        None
    };

    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    println!("Lora server running at http://{local}");
    serve_with_admin(listener, db, admin_config).await
}
