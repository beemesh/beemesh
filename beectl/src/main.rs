use beectl::{apply_file, delete_file};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "beectl", about = "beemesh CLI")]
struct Cli {
    /// REST API base URL (can also be set via BEEMESH_API)
    #[arg(long = "api-url", env = "BEEMESH_API", value_name = "URL")]
    api_url: Option<String>,
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
    /// Delete a configuration from the cluster  
    Delete {
        /// Filename, e.g. -f ./pod.yaml
        #[arg(short = 'f', long = "file", value_name = "FILE")]
        file: PathBuf,
        /// Force deletion without confirmation
        #[arg(long = "force")]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let Cli { api_url, command } = cli;

    match command {
        Commands::Apply { file } => {
            let _task_id = apply_file(file, api_url.as_deref()).await?;
        }
        Commands::Delete { file, force } => {
            let _task_id = delete_file(file, force, api_url.as_deref()).await?;
        }
    }

    Ok(())
}
