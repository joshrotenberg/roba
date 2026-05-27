use clap::Parser;
use roba::cli::Cli;
use roba::render::Style;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // Snapshot whether the user asked for plain BEFORE dispatch
    // consumes the AskArgs -- error styling needs to honor it too.
    let plain = cli.ask.plain;
    if let Err(err) = roba::dispatch(cli).await {
        let style = if plain { Style::plain() } else { Style::detect_for_error() };
        roba::render::print_error(&format!("{err:#}"), &style);
        std::process::exit(roba::classify_exit_code(&err));
    }
}
