//! The session backend seam.
//!
//! [`SessionBackend`] is the trait the actor programs against; the MCP surface
//! reads only its outputs, so the backend can swap without any client-facing
//! change. [`DuplexBackend`] is the warm implementation: a claude-wrapper
//! [`Conversation`] over one persistent `DuplexSession`. `Conversation` gives
//! cumulative cost / turn count / history and an optional budget ceiling, so
//! the actor keeps no hand-rolled accounting. A future `PseudoBackend` (fresh
//! `claude -p --resume` per turn, via roba-core `engine::run`) would drop in
//! behind the same trait.

use anyhow::Result;
use claude_wrapper::duplex::{DuplexOptions, DuplexSession, InboundEvent};
use claude_wrapper::{BudgetTracker, Claude, Conversation};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast::{Receiver, error::RecvError};

use crate::config::ServerConfig;

/// Backend-agnostic outcome of one turn (a superset of `QueryResult` /
/// `TurnResult`). `session_id` / `cost_usd` are the honest per-turn figures a
/// real server surfaces in the result `_meta` (a later pass); the current
/// surface reports cumulative figures via `status` instead.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub text: String,
    pub structured: Option<Value>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub is_error: bool,
}

/// The one seam. The actor is generic over this trait and calls `turn`;
/// swapping warm <-> pseudo-duplex never touches the actor, queue, or MCP
/// surface. `turn` returns an explicit `Send` future (not a native `async fn`)
/// so the actor can drive it from a `tokio::spawn`ed task.
pub trait SessionBackend: Send {
    /// Run one turn and return its outcome.
    fn turn(
        &mut self,
        prompt: String,
    ) -> impl std::future::Future<Output = Result<TurnOutcome>> + Send;
    /// The claude session id, once the session has been spawned.
    fn session_id(&self) -> Option<&str>;
    /// Cumulative spend across all turns on this session.
    fn total_cost_usd(&self) -> f64;
    /// Turns completed on this session.
    fn total_turns(&self) -> u32;
}

/// Warm backend: a [`Conversation`] over one persistent `claude` child, spawned
/// lazily on the first turn so the session id is minted then.
pub struct DuplexBackend {
    claude: Arc<Claude>,
    model: Option<String>,
    /// Inline JSON schema. `Some` => structured session (fixed at spawn);
    /// `None` => a prose session. Per-session, not per-call.
    schema: Option<String>,
    /// Optional session spend ceiling (USD). `send` fails fast with
    /// `Error::BudgetExceeded` once exceeded.
    max_usd: Option<f64>,
    /// Path to a generated `--mcp-config` pointing the child at the inward
    /// (south) MCP server. `Some` wires the reflexive surface; `None` omits it.
    mcp_config: Option<String>,
    /// Full-auto: bypass permission checks (all tools). Off = read-only posture.
    full_auto: bool,
    /// Writable posture: add Edit + Write to the read-only base (ignored under
    /// full_auto).
    writable: bool,
    /// Extra allowed tool patterns layered on the posture (e.g. `Bash(gh:*)`).
    allow_tools: Vec<String>,
    /// Tool patterns to deny, applied on top of any posture.
    deny_tools: Vec<String>,
    conversation: Option<Conversation>,
}

impl DuplexBackend {
    /// Build the backend from the resolved config plus the generated inward
    /// `--mcp-config` path (if the reflexive surface is wired).
    pub fn new(claude: Arc<Claude>, config: &ServerConfig, mcp_config: Option<String>) -> Self {
        Self {
            claude,
            model: config.model.clone(),
            schema: config.schema.clone(),
            max_usd: config.max_usd,
            mcp_config,
            full_auto: config.full_auto,
            writable: config.writable,
            allow_tools: config.allow_tools.clone(),
            deny_tools: config.deny_tools.clone(),
            conversation: None,
        }
    }

    async fn ensure_spawned(&mut self) -> Result<()> {
        if self.conversation.is_some() {
            return Ok(());
        }
        tracing::info!(
            model = ?self.model,
            structured = self.schema.is_some(),
            max_usd = ?self.max_usd,
            "session: spawning warm claude child",
        );
        let mut opts = DuplexOptions::default();
        if let Some(model) = &self.model {
            opts = opts.model(model.clone());
        }
        // Posture. Full-auto bypasses permission checks (all tools). Otherwise
        // safe-by-default: read-only tools, plus the inward mcp__roba__* tools
        // when the reflexive surface is wired. A later pass maps roba-core
        // `Permissions` here (and elicitation can gate escalation).
        if self.full_auto {
            opts = opts.dangerously_skip_permissions();
        } else {
            let mut allowed = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
            if self.writable {
                allowed.push("Edit".to_string());
                allowed.push("Write".to_string());
            }
            if self.mcp_config.is_some() {
                allowed.push("mcp__roba".to_string());
            }
            // Extra patterns (e.g. Bash(gh:*)) for a read-plus-review posture.
            allowed.extend(self.allow_tools.iter().cloned());
            opts = opts.allowed_tools(allowed);
        }
        // Deny patterns apply on top of any posture (carve-outs).
        if !self.deny_tools.is_empty() {
            opts = opts.disallowed_tools(self.deny_tools.clone());
        }
        // The inward surface is wired regardless of posture (strict: the child's
        // MCP surface is exactly our server).
        if let Some(path) = &self.mcp_config {
            opts = opts.mcp_config(path.clone()).strict_mcp_config();
        }
        if let Some(schema) = &self.schema {
            // Per-session structured mode: --json-schema is fixed for the child.
            opts = opts.json_schema(schema.clone());
        }
        let session = DuplexSession::spawn(&self.claude, opts).await?;
        // Surface the child's tool-by-tool activity in our own trace stream
        // (subscribe before the first turn so no events are missed).
        spawn_event_logger(session.subscribe());
        let mut conversation = Conversation::new(session);
        if let Some(max) = self.max_usd {
            conversation = conversation.with_budget(BudgetTracker::builder().max_usd(max).build());
        }
        self.conversation = Some(conversation);
        // The session id is minted by claude on the first turn (not at spawn),
        // so it is logged on `turn: complete`, not here.
        tracing::info!("session: spawned warm claude child");
        Ok(())
    }
}

impl SessionBackend for DuplexBackend {
    async fn turn(&mut self, prompt: String) -> Result<TurnOutcome> {
        self.ensure_spawned().await?;
        let conversation = self.conversation.as_mut().expect("spawned above");
        // `send` returns Err(BudgetExceeded) before touching the session once a
        // ceiling is hit; that propagates and the tool surfaces it as an error.
        let turn = conversation.send(prompt).await?;
        Ok(TurnOutcome {
            text: turn.result_text().unwrap_or_default().to_string(),
            structured: turn
                .result
                .get("structured_output")
                .filter(|v| !v.is_null())
                .cloned(),
            session_id: turn.session_id().map(str::to_string),
            cost_usd: turn.total_cost_usd(),
            is_error: turn
                .result
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn session_id(&self) -> Option<&str> {
        self.conversation
            .as_ref()
            .and_then(Conversation::session_id)
    }

    fn total_cost_usd(&self) -> f64 {
        self.conversation
            .as_ref()
            .map_or(0.0, Conversation::total_cost_usd)
    }

    fn total_turns(&self) -> u32 {
        self.conversation
            .as_ref()
            .map_or(0, Conversation::total_turns)
    }
}

/// Drive a background task that logs the child's tool-by-tool activity from the
/// session's event broadcast. Runs until the session closes. A slow consumer
/// drops events (`Lagged`) rather than back-pressuring the session; that is a
/// best-effort trace, not a durable log (the session JSONL is the record).
fn spawn_event_logger(mut events: Receiver<InboundEvent>) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(InboundEvent::Assistant(msg)) => log_tool_calls(&msg),
                Ok(_) => {} // SystemInit / StreamEvent / User / Other: skip
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "child event stream lagged");
                }
            }
        }
    });
}

/// Emit one `child: tool` trace line per tool-use block in an assistant message.
fn log_tool_calls(assistant: &Value) {
    let Some(content) = assistant
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
        let detail = tool_detail(name, block.get("input"));
        tracing::info!(tool = name, detail = %detail, "child: tool");
    }
}

/// A short human-readable summary of a tool-use input for the trace line.
fn tool_detail(name: &str, input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let take = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    match name {
        "Bash" => take(
            input.get("command").and_then(Value::as_str).unwrap_or(""),
            160,
        ),
        "Read" | "Edit" | "Write" | "NotebookEdit" => input
            .get("file_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "Glob" | "Grep" => take(
            input.get("pattern").and_then(Value::as_str).unwrap_or(""),
            80,
        ),
        _ => take(&input.to_string(), 120),
    }
}
