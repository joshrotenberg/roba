use clap::Parser;
use roba::cli::{Cli, ConfigCmd, SubCommand};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = wants_json(&cli);

    if let Err(error) = roba::dispatch(cli).await {
        if let Some(unusable) = error.downcast_ref::<roba::UnusableResultError>() {
            eprintln!("roba: {} (exit {})", unusable.note(), unusable.code());
            std::process::exit(unusable.code());
        }

        let exit_code = roba::classify_exit_code(&error);
        if json {
            eprintln!("{}", roba::error::render_json(&error, exit_code));
        } else {
            eprintln!("error: {}", roba::error::render_human_error(&error));
        }
        std::process::exit(exit_code);
    }
}

fn wants_json(cli: &Cli) -> bool {
    match &cli.command {
        Some(SubCommand::Run(args)) => args.json,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Effective(args),
        }) => args.json,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Survey(args),
        }) => args.json,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Propose(args),
        }) => args.json,
        _ => false,
    }
}
