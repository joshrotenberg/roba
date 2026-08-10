//! Run-scoped REPL over [`roba_core::RunHandle`].
//!
//! A bare line starts a suspended run or steers a running one. Slash commands
//! provide explicit control. This crate owns parsing and stream adaptation,
//! not run state.

use std::fmt;

use roba_core::{Prompt, RunHandle, RunState};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

const HELP: &str = "commands: /start TEXT, /steer TEXT, /status, /wait, /cancel, /help, /quit; a bare line starts a suspended run or steers a running run";

/// One parsed REPL command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplResponse {
    /// Text to render, if any. Snapshots are JSON for scriptable inspection.
    pub output: Option<String>,
    /// Whether the adapter should stop reading input.
    pub exit: bool,
}

impl ReplResponse {
    fn output(value: impl Into<String>) -> Self {
        Self {
            output: Some(value.into()),
            exit: false,
        }
    }

    fn exit() -> Self {
        Self {
            output: None,
            exit: true,
        }
    }
}

/// Cloneable REPL controller for a single run.
#[derive(Clone)]
pub struct Repl {
    handle: RunHandle,
}

impl Repl {
    pub fn new(handle: RunHandle) -> Self {
        Self { handle }
    }

    /// Execute one input line.
    pub async fn command(&self, line: &str) -> Result<ReplResponse, ReplError> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(ReplResponse {
                output: None,
                exit: false,
            });
        }
        if line == "/help" {
            return Ok(ReplResponse::output(HELP));
        }
        if matches!(line, "/quit" | "/exit") {
            return Ok(ReplResponse::exit());
        }
        if line == "/status" {
            return snapshot(self.handle.status().await);
        }
        if line == "/wait" {
            return snapshot(self.handle.wait().await);
        }
        if line == "/cancel" {
            self.handle.cancel().await?;
            return snapshot(self.handle.status().await);
        }
        if let Some(text) = argument(line, "/start") {
            self.handle.start(Prompt::new(text)?).await?;
            return snapshot(self.handle.status().await);
        }
        if let Some(text) = argument(line, "/steer") {
            self.handle.steer(Prompt::new(text)?).await?;
            return snapshot(self.handle.status().await);
        }
        if line.starts_with('/') {
            return Err(ReplError::UnknownCommand(line.to_string()));
        }

        let prompt = Prompt::new(line)?;
        match self.handle.status().await.state {
            RunState::Suspended => self.handle.start(prompt).await?,
            RunState::Running | RunState::Waiting => self.handle.steer(prompt).await?,
            state => return Err(ReplError::CannotPrompt(state)),
        }
        snapshot(self.handle.status().await)
    }

    /// Drive newline-delimited input and output. Terminal decoration and line
    /// editing can wrap this method without changing command semantics.
    pub async fn run<R, W>(&self, reader: R, mut writer: W) -> Result<(), ReplError>
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            match self.command(&line).await {
                Ok(response) => {
                    if let Some(output) = response.output {
                        writer.write_all(output.as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        writer.flush().await?;
                    }
                    if response.exit {
                        break;
                    }
                }
                Err(error) => {
                    writer
                        .write_all(format!("error: {error}\n").as_bytes())
                        .await?;
                    writer.flush().await?;
                }
            }
        }
        Ok(())
    }
}

fn argument<'a>(line: &'a str, command: &str) -> Option<&'a str> {
    line.strip_prefix(command)
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim)
}

fn snapshot(value: roba_core::RunSnapshot) -> Result<ReplResponse, ReplError> {
    Ok(ReplResponse::output(serde_json::to_string(&value)?))
}

/// REPL input, lifecycle, or stream error.
#[derive(Debug)]
pub enum ReplError {
    Prompt(roba_core::PromptError),
    Control(roba_core::RunControlError),
    UnknownCommand(String),
    CannotPrompt(RunState),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ReplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prompt(error) => error.fmt(f),
            Self::Control(error) => error.fmt(f),
            Self::UnknownCommand(command) => write!(f, "unknown command {command:?}; {HELP}"),
            Self::CannotPrompt(state) => {
                write!(f, "cannot send a bare prompt while run is {state:?}")
            }
            Self::Io(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReplError {}

impl From<roba_core::PromptError> for ReplError {
    fn from(error: roba_core::PromptError) -> Self {
        Self::Prompt(error)
    }
}

impl From<roba_core::RunControlError> for ReplError {
    fn from(error: roba_core::RunControlError) -> Self {
        Self::Control(error)
    }
}

impl From<std::io::Error> for ReplError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ReplError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use roba_core::{
        AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
        ProviderId, Run, RunOutcome, RunSpec, SessionHandle, TurnRequest,
    };

    use super::*;

    struct FakeProvider;

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::new("fake").unwrap(),
                        id: "session-1".to_string(),
                    }),
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
    }

    fn repl() -> Repl {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        Repl::new(run.handle())
    }

    #[tokio::test]
    async fn a_bare_line_starts_a_suspended_run() {
        let repl = repl();
        let response = repl.command("hello").await.unwrap();
        assert!(response.output.unwrap().contains("\"state\""));
        let terminal = repl.command("/wait").await.unwrap();
        let json: serde_json::Value =
            serde_json::from_str(terminal.output.as_deref().unwrap()).unwrap();
        assert_eq!(json["state"], "completed");
        assert_eq!(json["last_outcome"]["output"], "hello");
    }

    #[tokio::test]
    async fn help_unknown_and_quit_are_adapter_only() {
        let repl = repl();
        assert!(repl.command("/help").await.unwrap().output.is_some());
        assert!(matches!(
            repl.command("/wat").await.unwrap_err(),
            ReplError::UnknownCommand(_)
        ));
        assert!(repl.command("/quit").await.unwrap().exit);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                repl.command("/status")
                    .await
                    .unwrap()
                    .output
                    .as_deref()
                    .unwrap()
            )
            .unwrap()["state"],
            "suspended"
        );
    }
}
