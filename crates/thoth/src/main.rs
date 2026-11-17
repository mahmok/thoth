use color_eyre::eyre::eyre;
use thoth_core::composition;

fn main() -> color_eyre::Result<()> {
    if asks_for_version() {
        println!("thoth {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    run()
}

/// `--version` and `-V` are the only arguments the binary answers: everything else,
/// including no arguments at all, starts the terminal UI.
fn asks_for_version() -> bool {
    std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V")
}

#[tokio::main]
async fn run() -> color_eyre::Result<()> {
    let harness = composition::start().await.map_err(|e| eyre!("{e:#}"))?;
    thoth_terminal::run(harness)
}
