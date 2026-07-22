use clap::Parser;
use roba::cli::{Cli, ConfigCmd, SubCommand, WorktreeCmd};
use roba::render::Style;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // Snapshot whether the user asked for plain BEFORE dispatch
    // consumes the AskArgs -- error styling needs to honor it too.
    let plain = cli.ask.plain;
    let json = wants_json(&cli);
    // A detached child records that it started, so `roba show` can tell
    // "still running" from "never started" (#441). No-op without the
    // ROBA_RECEIPT the detaching parent sets, i.e. for every ordinary run.
    roba::receipt::start();
    if let Err(err) = roba::dispatch(cli).await {
        let exit_code = roba::classify_exit_code(&err);
        if json {
            eprintln!("{}", roba::error::render_json(&err, exit_code));
        } else {
            let style = if plain {
                Style::plain()
            } else {
                Style::detect_for_error()
            };
            roba::render::print_error(&roba::error::render_human_error(&err), &style);
            // Additive: an actionable hint for the detectable
            // first-run failures (claude missing / unauthenticated).
            // Printed after the primary error, never instead of it.
            if let Some(hint) = roba::error::hint_for_error(&err) {
                roba::render::print_meta(&hint, &style);
            }
        }
        // The error seam: record the typed code before exiting so an
        // observer sees the real outcome, not a reconstructed success.
        roba::receipt::finish(exit_code);
        std::process::exit(exit_code);
    }
    // The success seam. `exit_unusable` (code 6) records its own.
    roba::receipt::finish(0);
}

/// True when any explicit `--json` flag on the invocation asked for
/// structured output. Drives whether the error path emits a JSON
/// envelope on stderr instead of plain anyhow text. We snapshot from
/// the parsed CLI before dispatch so the decision survives error
/// bubble-up without plumbing args back from each runner.
fn wants_json(cli: &Cli) -> bool {
    if cli.ask.json {
        return true;
    }
    match &cli.command {
        Some(SubCommand::History(args)) => args.json,
        Some(SubCommand::Last(args)) => args.json,
        Some(SubCommand::Cost(args)) => args.json,
        Some(SubCommand::Doctor(args)) => args.json,
        Some(SubCommand::Worktree {
            cmd: WorktreeCmd::List(args),
        }) => args.json,
        Some(SubCommand::Show(args)) => args.json,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Lint(args),
        }) => args.json,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Show(args),
        }) => args.json,
        _ => false,
    }
}
