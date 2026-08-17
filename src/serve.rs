//! Foreground stdio host for one hot provider-neutral agent.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use roba_mcp::StdioBinding;

use crate::bounded;
use crate::cli::ServeArgs;

/// Construct one configured agent and serve its control contract over stdio.
pub async fn run(args: ServeArgs) -> Result<()> {
    let agent = bounded::build_agent(&args.agent)?;
    let binding = StdioBinding::new(agent);
    let handle = binding.handle();
    let stdin_is_terminal = std::io::stdin().is_terminal();

    let binding_run = binding.run();
    tokio::pin!(binding_run);
    let shutdown_signal = shutdown_signal(stdin_is_terminal);
    tokio::pin!(shutdown_signal);

    tokio::select! {
        result = &mut binding_run => {
            result.context("stdio MCP binding failed")
        }
        signal = &mut shutdown_signal => {
            // StdioBindingHandle initiates Tower's graceful drain. Keep
            // polling the binding future so in-flight responses and the
            // active provider are drained rather than abandoned.
            let _ = handle.shutdown();
            let result = binding_run.await;
            signal.context("failed to monitor process shutdown signals")?;
            result.context("stdio MCP binding failed")
        }
    }
}

fn sigint_requests_shutdown(stdin_is_terminal: bool) -> bool {
    stdin_is_terminal
}

#[cfg(unix)]
async fn shutdown_signal(stdin_is_terminal: bool) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        tokio::select! {
            received = terminate.recv() => {
                received.ok_or_else(|| std::io::Error::other("SIGTERM listener closed"))?;
                return Ok(());
            }
            received = interrupt.recv() => {
                received.ok_or_else(|| std::io::Error::other("SIGINT listener closed"))?;
                if sigint_requests_shutdown(stdin_is_terminal) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(windows)]
async fn shutdown_signal(stdin_is_terminal: bool) -> std::io::Result<()> {
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    loop {
        interrupt
            .recv()
            .await
            .ok_or_else(|| std::io::Error::other("Ctrl-C listener closed"))?;
        if sigint_requests_shutdown(stdin_is_terminal) {
            return Ok(());
        }
    }
}

#[cfg(not(any(unix, windows)))]
async fn shutdown_signal(stdin_is_terminal: bool) -> std::io::Result<()> {
    loop {
        tokio::signal::ctrl_c().await?;
        if sigint_requests_shutdown(stdin_is_terminal) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigint_stops_a_direct_terminal_but_not_a_piped_mcp_server() {
        assert!(sigint_requests_shutdown(true));
        assert!(!sigint_requests_shutdown(false));
    }
}
