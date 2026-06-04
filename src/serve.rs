// SPIKE: Minimal roba serve MCP server using tower-mcp.
//
// Spike goals validated here:
//
// 1. tower-mcp API fit -- ToolBuilder + ResourceBuilder + StdioTransport all
//    compose cleanly. The extractor-based handler pattern (input type that
//    derives JsonSchema) maps directly onto DispatchArgs. No friction.
//
// 2. send_prompt tool -- VALIDATED. Real dispatch via execute_json wired.
//    session_id timing confirmed: session_id is returned in QueryResult after
//    execute_json completes -- fine for synchronous use. Orphanable jobs
//    (fire-and-forget) require a different pattern (streaming or a channel);
//    deferred to the real impl.
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
use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};
use claude_wrapper::{Claude, QueryCommand};
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
/// an existing one. `model` overrides the default claude model. `system_prompt`
/// replaces the default system prompt for this call.
#[derive(Debug, Deserialize, JsonSchema)]
struct SendPromptInput {
    /// The prompt text to send to claude.
    prompt: String,

    /// An existing roba/claude session id to continue. Optional.
    #[serde(default)]
    session_id: Option<String>,

    /// Override the claude model for this call. Optional.
    #[serde(default)]
    model: Option<String>,

    /// Replace the default system prompt for this call. Optional.
    #[serde(default)]
    system_prompt: Option<String>,
}

/// Response shape for the `send_prompt` tool.
#[derive(Debug, Serialize)]
struct SendPromptOutput {
    result: String,
    session_id: String,
    /// The model that responded to this prompt.
    model: String,
}

// ============================================================================
// Dispatch
// ============================================================================

/// Call claude via execute_json and return the result, bypassing run_ask's
/// stdout-writing output path.
async fn dispatch_for_mcp(input: &SendPromptInput) -> anyhow::Result<SendPromptOutput> {
    let claude = Claude::builder().build()?;
    let name = crate::session::derive_session_name(&input.prompt);
    let mut cmd = QueryCommand::new(input.prompt.clone())
        .name(name)
        .prompt_via_stdin(true);

    if let Some(id) = &input.session_id
        && !id.is_empty()
    {
        cmd = cmd.resume(id.clone());
    }

    if let Some(m) = &input.model
        && !m.is_empty()
    {
        cmd = cmd.model(m.clone());
    }

    if let Some(s) = &input.system_prompt
        && !s.is_empty()
    {
        cmd = cmd.system_prompt(s.clone());
    }

    // Safe readonly defaults: only allow read-only tools.
    cmd = cmd.allowed_tools(vec![
        "Read".to_string(),
        "Glob".to_string(),
        "Grep".to_string(),
    ]);

    let result = cmd.execute_json(&claude).await?;

    // The model field is not a typed QueryResult field; it arrives in the
    // extra map from the CLI's JSON output.
    let model = result
        .extra
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(SendPromptOutput {
        result: result.result,
        session_id: result.session_id,
        model,
    })
}

// ============================================================================
// Server entry point
// ============================================================================

/// Run the MCP server. Called from `crate::dispatch` when the `Serve`
/// subcommand is matched.
pub async fn run_serve(args: ServeArgs) -> Result<()> {
    // -- send_prompt tool ------------------------------------------------------
    let send_prompt = ToolBuilder::new("send_prompt")
        .title("Send Prompt")
        .description(
            "Send a prompt to claude via roba and return the answer. \
             Supply session_id to continue an existing session. \
             Supply model to override the default claude model. \
             Supply system_prompt to replace the default system prompt.",
        )
        .handler(|input: SendPromptInput| async move {
            match dispatch_for_mcp(&input).await {
                Ok(output) => match serde_json::to_string(&output) {
                    Ok(json) => Ok(CallToolResult::text(json)),
                    Err(e) => Ok(CallToolResult::error(format!("serialization error: {e}"))),
                },
                Err(e) => Ok(CallToolResult::error(format!("dispatch error: {e}"))),
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

    // -- roba://sessions resource -----------------------------------------------
    //
    // Dynamic resource: reads up to 10 most recent sessions from history.
    // Content is snapshotted at server startup (static text resource).
    let sessions_json = {
        match HistoryRoot::home() {
            Err(e) => {
                serde_json::json!({"error": format!("could not locate history: {e}")}).to_string()
            }
            Ok(root) => {
                let opts = ListOptions {
                    limit: Some(10),
                    offset: 0,
                    include_empty: false,
                    sort: ListSort::RecencyDesc,
                };
                match root.list_sessions_with(None, &opts) {
                    Err(e) => serde_json::json!({"error": format!("could not list sessions: {e}")})
                        .to_string(),
                    Ok(sessions) => {
                        let items: Vec<serde_json::Value> = sessions
                            .iter()
                            .map(|s| {
                                serde_json::json!({
                                    "session_id": s.session_id,
                                    "project_slug": s.project_slug,
                                    "title": s.title,
                                })
                            })
                            .collect();
                        serde_json::to_string_pretty(&items).unwrap_or_else(|e| {
                            serde_json::json!({"error": format!("serialization error: {e}")})
                                .to_string()
                        })
                    }
                }
            }
        }
    };

    let sessions_resource = ResourceBuilder::new("roba://sessions")
        .name("roba recent sessions")
        .description(
            "Returns up to 10 most recent roba/claude sessions across all projects, \
             snapshotted at server startup.",
        )
        .mime_type("application/json")
        .text(sessions_json);

    // -- Router ----------------------------------------------------------------
    let router = McpRouter::new()
        .server_info("roba", env!("CARGO_PKG_VERSION"))
        .instructions(
            "roba MCP server: send prompts through roba's dispatch pipeline \
             and read server status.",
        )
        .tool(send_prompt)
        .resource(status)
        .resource(sessions_resource);

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
