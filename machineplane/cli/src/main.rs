use clap::{Parser, Subcommand};
use std::path::PathBuf;
use serde_json::Value as JsonValue;
use serde_yaml;
use reqwest;
use std::env;

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
    let cli = Cli::parse();

    match cli.command {
        Commands::Apply { file } => apply_file(file).await?,
    }

    Ok(())
}

async fn apply_file(path: PathBuf) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }

    // Read file contents
    let contents = tokio::fs::read_to_string(&path).await?;

    // Minimal inspection: find kind/name fields for YAML-like files
    let mut kind = None;
    let mut name = None;

    for line in contents.lines() {
        let l = line.trim();
        if l.starts_with("kind:") && kind.is_none() {
            kind = Some(l[5..].trim().to_string());
        }
        if l.starts_with("name:") && name.is_none() {
            name = Some(l[5..].trim().to_string());
        }
        if kind.is_some() && name.is_some() {
            break;
        }
    }

    println!("Applying file: {}", path.display());

    match (kind, name) {
        (Some(k), Some(n)) => println!("Detected resource: kind='{}' name='{}'", k, n),
        (Some(k), None) => println!("Detected resource: kind='{}' (no name found)", k),
        _ => println!("No top-level kind/name detected; sending raw payload..."),
    }

    // Send manifest to API
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    let url = format!("{}/tenant/{}/apply", base.trim_end_matches('/'), tenant);

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(manifest_json.to_string())
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;

    println!("API response: {}\n{}", status, body);

    if !status.is_success() {
        anyhow::bail!("apply failed: {}", status);
    }

    println!("Apply succeeded");

    Ok(())
}
