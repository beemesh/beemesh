use machine::{Cli, start_machine};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let handles = start_machine(cli).await?;
    if !handles.is_empty() {
        let _ = futures::future::join_all(handles).await;
    }
    Ok(())
}
