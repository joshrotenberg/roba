use clap::Parser;
use roba::cli::{Cli, SubCommand, WorktreeCmd};
use roba::render::Style;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // Snapshot whether the user asked for plain BEFORE dispatch
    // consumes the AskArgs -- error styling needs to honor it too.
    let plain = cli.ask.plain;
    let json = wants_json(&cli);
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
            roba::render::print_error(&format!("{err:#}"), &style);
            // Additive: an actionable hint for the detectable
            // first-run failures (claude missing / unauthenticated).
            // Printed after the primary error, never instead of it.
            if let Some(hint) = roba::error::hint_for_error(&err) {
                roba::render::print_meta(&hint, &style);
            }
        }
        std::process::exit(exit_code);
    }
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
        Some(SubCommand::Cost(args)) => args.json,
        Some(SubCommand::Doctor(args)) => args.json,
        Some(SubCommand::Worktree {
            cmd: WorktreeCmd::List(args),
        }) => args.json,
        Some(SubCommand::Show(args)) => args.json,
        _ => false,
    }
}
