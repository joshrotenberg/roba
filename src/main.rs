use clap::Parser;
use cwr::cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(err) = cwr::dispatch(cli).await {
        eprintln!("error: {err:#}");
        std::process::exit(cwr::classify_exit_code(&err));
    }
}
