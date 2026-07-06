//! roba-server: a single warm claude session exposed over MCP.
//!
//! # Architecture
//!
//! **One process = one warm session = one MCP endpoint.** There is no session
//! id in the protocol and no session pool: the process *is* the session. Want
//! N sessions? Run N processes (the OS is the pool). This deletes the routing /
//! ledger / eviction layer that sinks larger "session server" designs.
//!
//! ```text
//!   MCP client (stdio)
//!        |  tools/call: prompt / status
//!        v
//!   tools::router  ---- one SessionHandle (clone) per tool
//!        |  handle.prompt(text).await
//!        v
//!   session actor  ---- one mpsc mailbox = the per-session mutex (FIFO,
//!        |               one turn at a time; concurrent calls queue)
//!        v
//!   SessionBackend (trait)
//!        |  DuplexBackend: a claude-wrapper Conversation over one warm
//!        v               DuplexSession (held-open child, context across turns)
//!   claude
//! ```
//!
//! The [`SessionBackend`](backend::SessionBackend) trait is the seam: the warm
//! [`DuplexBackend`] today, a pseudo-duplex backend
//! (fresh `claude -p --resume` per turn via roba-core `engine::run`) as a
//! future drop-in that the MCP surface never sees.
//!
//! # Modes (launch config, fixed for the session)
//!
//! - model (`ROBA_MODEL`)
//! - per-session structured output (`ROBA_SCHEMA`, an inline JSON schema):
//!   structured because `DuplexSession`'s `--json-schema` is fixed at spawn and
//!   MCP `outputSchema` is per-tool. A different schema means a different
//!   process.
//! - optional session spend ceiling (`ROBA_MAX_USD`), a `Conversation` budget.

pub mod backend;
pub mod config;
pub mod session;
pub mod tools;

use anyhow::Result;
use claude_wrapper::Claude;
use std::sync::Arc;
use tower_mcp::StdioTransport;

use crate::backend::DuplexBackend;
use crate::config::ServerConfig;
use crate::session::spawn_session_actor;

/// Build the session actor + MCP router for `config` and serve it over stdio.
///
/// Blocks until the transport closes (the client disconnects / stdin EOF).
pub async fn serve(config: ServerConfig) -> Result<()> {
    let structured = config.structured();
    let claude = Arc::new(Claude::builder().build()?);
    let backend = DuplexBackend::new(claude, config.model, config.schema, config.max_usd);
    let handle = spawn_session_actor(backend);
    let router = tools::router(handle, structured);
    StdioTransport::new(router).run().await?;
    Ok(())
}
