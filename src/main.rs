use clap::Parser;
use cwr::cli::Cli;
use cwr::render::Style;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // Snapshot whether the user asked for plain BEFORE dispatch
    // consumes the AskArgs -- error styling needs to honor it too.
    let plain = cli.ask.plain;
    if let Err(err) = cwr::dispatch(cli).await {
        let style = if plain { Style::plain() } else { Style::detect_for_error() };
        cwr::render::print_error(&format!("{err:#}"), &style);
        std::process::exit(cwr::classify_exit_code(&err));
    }
}
