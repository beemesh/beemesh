use clap::Parser;
use machineplane::{Cli, start_machineplane};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let handles = start_machineplane(cli).await?;
    if !handles.is_empty() {
        let _ = futures::future::join_all(handles).await;
    }
    Ok(())
}
