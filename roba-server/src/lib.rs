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
pub mod inward;
pub mod session;
pub mod tools;

use anyhow::Result;
use claude_wrapper::Claude;
use std::sync::{Arc, Mutex};
use tower_mcp::StdioTransport;

use crate::backend::DuplexBackend;
use crate::config::ServerConfig;
use crate::inward::InwardContext;
use crate::session::{SessionStatus, spawn_session_actor};

/// Build the session actor + MCP router for `config` and serve it over stdio.
///
/// Blocks until the transport closes (the client disconnects / stdin EOF).
///
/// When `config.inward` is set (the default) the reflexive south surface is
/// wired first: an in-process HTTP MCP server on an ephemeral localhost port,
/// and a generated `--mcp-config` handed to the child so the running agent can
/// introspect its own context. The status is shared so the agent's `context`
/// tool and the north `status` tool report the same live figures.
pub async fn serve(config: ServerConfig) -> Result<()> {
    let structured = config.structured();
    // Shared live status: the actor writes it, the north `status` tool and the
    // inward `context` tool read it.
    let status = Arc::new(Mutex::new(SessionStatus::default()));

    // Inward surface (optional): bring it up before the child can spawn, and
    // keep the generated config file alive for the whole run.
    let _mcp_config_file;
    let mcp_config_path = if config.inward {
        let ctx = InwardContext {
            config: config.clone(),
            status: status.clone(),
        };
        let url = inward::spawn_server(ctx).await?;
        let file = inward::write_mcp_config(&url)?;
        let path = file.path().to_string_lossy().into_owned();
        _mcp_config_file = Some(file);
        Some(path)
    } else {
        _mcp_config_file = None;
        None
    };

    let claude = Arc::new(Claude::builder().build()?);
    let backend = DuplexBackend::new(
        claude,
        config.model,
        config.schema,
        config.max_usd,
        mcp_config_path,
        config.full_auto,
    );
    let handle = spawn_session_actor(backend, status);
    let router = tools::router(handle, structured);
    StdioTransport::new(router).run().await?;
    Ok(())
}
