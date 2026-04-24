use std::process::ExitCode;
use std::sync::Arc;

use lora_database::{Database, SnapshotAdmin};
use lora_server::config::{self, ConfigOutcome, ServerConfig};
use lora_server::{serve_with_admin, AdminConfig};

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
    let db = Arc::new(Database::in_memory());

    // Optional restore-from-snapshot at boot. A missing file is fine (fresh
    // process, nothing persisted yet); any other error is fatal because the
    // operator asked us to restore and we couldn't honour it.
    if let Some(path) = cfg.restore_from.as_ref() {
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
    }

    // Opt-in admin surface. Only wired when --snapshot-path was provided;
    // otherwise the /admin routes do not exist at all.
    let admin_config = cfg.snapshot_path.as_ref().map(|path| {
        println!(
            "Admin routes enabled: POST /admin/snapshot/{{save,load}} → {}",
            path.display()
        );
        AdminConfig {
            snapshot_path: path.clone(),
            admin: Arc::clone(&db) as Arc<dyn SnapshotAdmin>,
        }
    });

    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    println!("Lora server running at http://{local}");
    serve_with_admin(listener, db, admin_config).await
}
