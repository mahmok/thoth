use color_eyre::eyre::eyre;
use thoth_core::bootstrap;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let harness = bootstrap::start().await.map_err(|e| eyre!("{e:#}"))?;
    thoth_terminal::run(harness)
}
