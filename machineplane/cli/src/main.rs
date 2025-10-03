use clap::{Parser, Subcommand};
use std::path::PathBuf;
use cli::apply_file;

#[derive(Parser, Debug)]
#[command(name = "beemesh", about = "beemesh CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Apply a configuration file to the cluster
    Apply {
        /// Filename, e.g. -f ./pod.yaml
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        file: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Apply { file } => apply_file(file).await?,
    }

    Ok(())
}
