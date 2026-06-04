// SPIKE: Minimal roba serve MCP server using tower-mcp.
//
// Spike goals validated here:
//
// 1. tower-mcp API fit -- ToolBuilder + ResourceBuilder + StdioTransport all
//    compose cleanly. The extractor-based handler pattern (input type that
//    derives JsonSchema) maps directly onto DispatchArgs. No friction.
//
// 2. send_prompt tool -- accepts {prompt, session_id?} JSON, stubbed response
//    returns {result, session_id}. The real dispatch path (run_ask) is async
//    and needs Tokio; tower-mcp handlers ARE async, so this will wire up
//    without impedance mismatch.
//
// 3. roba://status resource -- static read-only resource; ResourceBuilder
//    wires with one call. Works exactly as expected.
//
// 4. Transport choice -- StdioTransport is the lowest-friction option for
//    the spike and works with Claude Code / Claude Desktop out of the box.
//    Unix socket transport requires the `unix` feature (which pulls in axum
//    + hyper); deferred to a follow-up spike or the real impl.
//
// Open questions / friction points:
//
// - session_id timing: the real run_ask dispatches to `claude -p` and the
//   session id is only known AFTER the call returns (or early in the stream
//   for --stream). The send_prompt response therefore can't include a real
//   session_id without either (a) streaming or (b) post-call enrichment.
//   For the spike this returns an empty string; for the real impl we'll
//   need to either stream the response or enrich after the fact.
//
// - State sharing: passing the result of run_ask back through a tower-mcp
//   tool handler requires State<T> injection or a channel. tower-mcp's
//   State<T> extractor (like axum's) is the right seam -- see the
//   ToolBuilder::with_state pattern in the tower-mcp docs.
//
// - Unix socket: the `unix` feature adds axum/hyper to the dep tree. For
//   the spike, stdio is fine. For the real impl, wire the default socket
//   path (`~/.local/state/roba/server.sock`) behind a `--stdio` / socket
//   toggle and enable the `unix` feature.
//
// - Error propagation: CallToolResult::error_text is the right shape for
//   dispatch failures; the anyhow::Error -> string coercion is one line.

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tower_mcp::{CallToolResult, McpRouter, ResourceBuilder, StdioTransport, ToolBuilder};

use crate::cli::ServeArgs;

// ============================================================================
// Tool input / output types
// ============================================================================

/// Input type for the `send_prompt` tool.
///
/// `session_id` is optional: omit to start a new session, supply to continue
/// an existing one. For the spike the field is accepted but not yet acted on
/// (the underlying run_ask wiring is stubbed).
#[derive(Debug, Deserialize, JsonSchema)]
struct SendPromptInput {
    /// The prompt text to send to claude.
    prompt: String,

    /// An existing roba/claude session id to continue. Optional.
    #[serde(default)]
    session_id: Option<String>,
}

/// Response shape for the `send_prompt` tool.
///
/// `session_id` will be populated by the real impl once session tracking is
/// wired through run_ask. For the spike it is always an empty string.
#[derive(Debug, Serialize)]
struct SendPromptOutput {
    result: String,
    session_id: String,
}

// ============================================================================
// Server entry point
// ============================================================================

/// Run the MCP server. Called from `crate::dispatch` when the `Serve`
/// subcommand is matched.
pub async fn run_serve(args: ServeArgs) -> Result<()> {
    // -- send_prompt tool ------------------------------------------------------
    //
    // SPIKE: the handler is stubbed. A real implementation would:
    //   1. Map SendPromptInput fields onto an AskArgs
    //   2. Call run_ask (or a lower-level dispatch function) and capture the
    //      result
    //   3. Extract the session id from the result and return it alongside the
    //      answer text
    //
    // The stubbed response exercises the full tool registration + transport
    // path without needing a live ANTHROPIC_API_KEY.
    let send_prompt = ToolBuilder::new("send_prompt")
        .title("Send Prompt")
        .description(
            "Send a prompt to claude via roba and return the answer. \
             Supply session_id to continue an existing session.",
        )
        .handler(|input: SendPromptInput| async move {
            // SPIKE: stub response. Replace with real run_ask dispatch.
            let output = SendPromptOutput {
                result: format!(
                    "[spike stub] roba would dispatch: {:?} (session: {:?})",
                    input.prompt, input.session_id
                ),
                // SPIKE: session_id is only known after run_ask returns.
                // For the real impl, extract from QueryResult.session_id.
                session_id: String::new(),
            };
            match serde_json::to_string(&output) {
                Ok(json) => Ok(CallToolResult::text(json)),
                Err(e) => Ok(CallToolResult::error(format!("serialization error: {e}"))),
            }
        })
        .build();

    // -- roba://status resource ------------------------------------------------
    //
    // Static read-only resource. Validates that ResourceBuilder wires
    // correctly alongside ToolBuilder -- no friction found.
    let status_json = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "server": "running"
    })
    .to_string();

    let status = ResourceBuilder::new("roba://status")
        .name("roba server status")
        .description("Returns the roba server version and running state.")
        .mime_type("application/json")
        .text(status_json);

    // -- Router ----------------------------------------------------------------
    let router = McpRouter::new()
        .server_info("roba", env!("CARGO_PKG_VERSION"))
        .instructions(
            "roba MCP server: send prompts through roba's dispatch pipeline \
             and read server status.",
        )
        .tool(send_prompt)
        .resource(status);

    // -- Transport selection ---------------------------------------------------
    //
    // SPIKE: stdio transport only. Unix socket (the production default) needs
    // the `unix` feature on tower-mcp (adds axum + hyper). For the spike,
    // stdio is sufficient to validate the shape.
    //
    // If --socket was supplied (and --stdio was not), we warn and fall through
    // to stdio for now.
    if args.socket.is_some() && !args.stdio {
        eprintln!(
            "roba serve: Unix socket transport not yet implemented in the spike. \
             Falling back to stdio. Pass --stdio explicitly to suppress this warning."
        );
    }

    eprintln!(
        "roba serve (spike): starting MCP server on stdio (version {})",
        env!("CARGO_PKG_VERSION")
    );

    StdioTransport::new(router).run().await?;

    Ok(())
}
