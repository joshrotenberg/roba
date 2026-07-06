//! Entry point: read the launch config from the environment and serve.

use anyhow::Result;
use roba_server::config::ServerConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let config = ServerConfig::from_env();
    // Metadata to stderr (stdout is the MCP JSON-RPC channel; keep it clean).
    eprintln!(
        "roba-server {}: stdio MCP up (model={:?}, structured={}, max_usd={:?})",
        env!("CARGO_PKG_VERSION"),
        config.model,
        config.structured(),
        config.max_usd,
    );
    roba_server::serve(config).await
}
