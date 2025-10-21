use clap::{Parser, Subcommand};
use cli::{apply_file, delete_file};
use std::path::PathBuf;

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

    match cli.command {
        Commands::Apply { file } => {
            let _task_id = apply_file(file).await?;
        }
        Commands::Delete { file, force } => {
            let _task_id = delete_file(file, force).await?;
        }
    }

    Ok(())
}
