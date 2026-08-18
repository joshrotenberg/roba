//! Foreground stdio binding for one logical agent.

use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tower_mcp::transport::StdioTransportHandle;
use tower_mcp::{ProtocolSupport, StdioTransport};

use crate::{AgentInstance, control_router};

const MAX_CONCURRENT_REQUESTS: usize = 16;

/// One foreground MCP stdio binding for a configured logical agent.
///
/// The binding owns the control projection. End of input, an explicit binding
/// shutdown, or [`crate::AGENT_SHUTDOWN_TOOL`] permanently stops and drains the
/// agent before the binding returns.
pub struct StdioBinding {
    agent: AgentInstance,
    transport: StdioTransport,
}

/// Opaque cloneable control handle for a running [`StdioBinding`].
#[derive(Clone)]
pub struct StdioBindingHandle {
    transport: StdioTransportHandle,
}

impl StdioBinding {
    /// Bind the operator/control MCP projection for one configured agent.
    pub fn new(agent: AgentInstance) -> Self {
        let transport = StdioTransport::new(control_router(agent.clone()))
            .protocol_support(ProtocolSupport::compiled())
            .max_concurrent_requests(MAX_CONCURRENT_REQUESTS);
        Self { agent, transport }
    }

    /// Return a handle that can gracefully stop this binding.
    pub fn handle(&self) -> StdioBindingHandle {
        StdioBindingHandle {
            transport: self.transport.handle(),
        }
    }

    /// Serve MCP over this process's stdin and stdout until the binding stops.
    ///
    /// This is a process-scoped foreground adapter. It consumes the binding
    /// and owns one bounded, detached blocking reader for standard input so a
    /// client that keeps its pipe open cannot pin Tokio runtime shutdown after
    /// logical agent shutdown. An embedding that continues after the binding
    /// returns should use [`Self::run_with_streams`] with an owned,
    /// controllable reader instead.
    pub async fn run(mut self) -> tower_mcp::Result<()> {
        let stdin = ThreadedStdin::new().map_err(|error| {
            tower_mcp::Error::Transport(format!("failed to start stdio input reader: {error}"))
        })?;
        let handle = self.transport.handle();
        let agent = self.agent.clone();
        supervise(
            agent,
            handle,
            self.transport.run_with_streams(stdin, tokio::io::stdout()),
        )
        .await
    }

    /// Serve MCP over caller-provided streams until the binding stops.
    ///
    /// This is primarily useful for embedding and end-to-end transport tests.
    pub async fn run_with_streams<R, W>(&mut self, reader: R, writer: W) -> tower_mcp::Result<()>
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        let handle = self.transport.handle();
        let agent = self.agent.clone();
        supervise(
            agent,
            handle,
            self.transport.run_with_streams(reader, writer),
        )
        .await
    }
}

impl StdioBindingHandle {
    /// Stop accepting input and gracefully drain in-flight MCP requests.
    pub fn shutdown(&self) -> tower_mcp::Result<()> {
        self.transport.shutdown()
    }
}

/// Async stdin backed by a detached ordinary thread instead of Tokio's
/// blocking pool.
///
/// A pending `tokio::io::stdin()` read cannot be cancelled while the parent
/// keeps its pipe open, so dropping the runtime after `agent.shutdown` can
/// wait forever for that read. An ordinary detached thread does not hold the
/// runtime or process open after this foreground binding has drained.
struct ThreadedStdin {
    chunks: tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    current: Vec<u8>,
    offset: usize,
}

impl ThreadedStdin {
    fn new() -> std::io::Result<Self> {
        const CHUNK_SIZE: usize = 8 * 1024;
        const QUEUE_DEPTH: usize = 8;

        let (sender, chunks) = tokio::sync::mpsc::channel(QUEUE_DEPTH);
        let reader = std::thread::Builder::new()
            .name("roba-stdio-input".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut stdin = stdin.lock();
                let mut chunk = [0; CHUNK_SIZE];
                loop {
                    match stdin.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(read) => {
                            if sender.blocking_send(Ok(chunk[..read].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            let _ = sender.blocking_send(Err(error));
                            break;
                        }
                    }
                }
            })?;
        drop(reader);
        Ok(Self {
            chunks,
            current: Vec::new(),
            offset: 0,
        })
    }
}

impl AsyncRead for ThreadedStdin {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.offset < this.current.len() {
                let available = &this.current[this.offset..];
                let read = available.len().min(output.remaining());
                output.put_slice(&available[..read]);
                this.offset += read;
                return Poll::Ready(Ok(()));
            }
            match this.chunks.poll_recv(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.current = chunk;
                    this.offset = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

async fn supervise<F>(
    agent: AgentInstance,
    transport: StdioTransportHandle,
    run: F,
) -> tower_mcp::Result<()>
where
    F: Future<Output = tower_mcp::Result<()>>,
{
    // Tower announces input shutdown before draining requests. Begin agent
    // shutdown at that boundary so a synchronous turn cannot keep the drain
    // waiting on provider work that only application shutdown would release.
    let stopping_agent = agent.clone();
    let stopping_transport = transport.clone();
    let stopping_supervisor = tokio::spawn(async move {
        stopping_transport.stopping().await;
        stopping_agent.shutdown().await;
    });

    // The MCP agent.shutdown tool changes application state first. Reflect
    // that state back into the owning transport so it exits after writing the
    // shutdown response and draining any other requests.
    let stopped_agent = agent.clone();
    let stopped_supervisor = tokio::spawn(async move {
        stopped_agent.wait_stopped().await;
        let _ = transport.shutdown();
    });

    let result = run.await;

    // Some transport failures return before Tower publishes its stopping
    // signal. This idempotent cleanup is therefore mandatory on every path.
    agent.shutdown().await;

    // A transport error can leave the passive stopping observer pending on a
    // still-owned transport. Do not leak either supervisor or its agent clone.
    stopping_supervisor.abort();
    stopped_supervisor.abort();
    let _ = stopping_supervisor.await;
    let _ = stopped_supervisor.await;

    result
}
