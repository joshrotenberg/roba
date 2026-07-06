//! The single serializing session actor.
//!
//! One session per process => one actor. The mpsc mailbox IS the per-session
//! mutex: turns run strictly one-at-a-time, FIFO, no interleaving. Concurrent
//! MCP `prompt` calls queue behind the in-flight turn. Durable state is claude's
//! on-disk session JSONL, not this task, so the actor is cheap and orphan-able.

use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

use crate::backend::{SessionBackend, TurnOutcome};

/// A read-only snapshot of the session for the `status` tool. The figures are
/// read from the backend (authoritative), not accumulated here.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SessionStatus {
    pub session_id: Option<String>,
    pub turns_completed: u32,
    pub cumulative_cost_usd: f64,
}

struct TurnRequest {
    prompt: String,
    reply: oneshot::Sender<Result<TurnOutcome>>,
}

/// Cloneable handle the MCP tool handlers hold. Enqueues turns onto the one
/// actor and reads the status snapshot.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<TurnRequest>,
    status: Arc<Mutex<SessionStatus>>,
}

impl SessionHandle {
    /// Enqueue a turn and await its result. Blocks (async) behind any in-flight
    /// turn -- that wait IS the queue.
    pub async fn prompt(&self, prompt: String) -> Result<TurnOutcome> {
        let (reply, rx) = oneshot::channel();
        tracing::debug!(prompt_len = prompt.len(), "turn: enqueued");
        self.tx
            .send(TurnRequest { prompt, reply })
            .await
            .map_err(|_| anyhow!("session actor is gone"))?;
        rx.await
            .map_err(|_| anyhow!("session actor dropped the turn"))?
    }

    /// The current status snapshot (instant; does not queue behind a turn).
    pub fn status(&self) -> SessionStatus {
        self.status.lock().expect("status mutex poisoned").clone()
    }
}

/// Spawn the actor over a backend and return its handle. `status` is owned by
/// the caller and shared with the inward MCP surface, so the running agent's
/// `context` tool reads the same live figures the actor writes.
pub fn spawn_session_actor<B>(mut backend: B, status: Arc<Mutex<SessionStatus>>) -> SessionHandle
where
    B: SessionBackend + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<TurnRequest>(64);
    let actor_status = status.clone();

    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let started = Instant::now();
            let outcome = backend.turn(req.prompt).await;
            let elapsed_ms = started.elapsed().as_millis();
            // Refresh the snapshot from the backend's authoritative figures.
            {
                let mut snapshot = actor_status.lock().expect("status mutex poisoned");
                snapshot.session_id = backend.session_id().map(str::to_string);
                snapshot.turns_completed = backend.total_turns();
                snapshot.cumulative_cost_usd = backend.total_cost_usd();
            }
            match &outcome {
                Ok(out) => tracing::info!(
                    turn = backend.total_turns(),
                    session_id = ?backend.session_id(),
                    elapsed_ms,
                    is_error = out.is_error,
                    cost_usd = ?out.cost_usd,
                    cumulative_cost_usd = backend.total_cost_usd(),
                    "turn: complete",
                ),
                Err(e) => tracing::warn!(elapsed_ms, error = %e, "turn: failed"),
            }
            // Receiver may be gone if the caller cancelled; ignore.
            let _ = req.reply.send(outcome);
        }
    });

    SessionHandle { tx, status }
}
