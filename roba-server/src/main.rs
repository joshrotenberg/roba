//! Entry point: read the launch config from the environment and serve.

use anyhow::Result;
use roba_server::config::ServerConfig;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let config = ServerConfig::from_env();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        model = ?config.model,
        structured = config.structured(),
        max_usd = ?config.max_usd,
        inward = config.inward,
        full_auto = config.full_auto,
        "roba-server: stdio MCP up",
    );
    roba_server::serve(config).await
}

/// Install the tracing subscriber, writing to **stderr only**: stdout is the
/// MCP JSON-RPC channel and must stay byte-clean (a stray log line corrupts the
/// protocol). Level via `RUST_LOG`; defaults to `roba_server=info,warn`.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("roba_server=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
