use std::process::ExitCode;
use std::sync::Arc;

use lora_database::Database;
use lora_server::config::{self, ConfigOutcome, ServerConfig};
use lora_server::serve;

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
    let addr = cfg.bind_addr();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    println!("Lora server running at http://{local}");
    serve(listener, db).await
}
