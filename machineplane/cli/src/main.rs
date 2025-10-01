use log::{info};
use clap::{Parser, Subcommand};
use dirs::home_dir;
use reqwest;
use rand::RngCore;
use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::fs;
use std::path::PathBuf;
use base64::Engine;
use crypto::{ensure_keypair_on_disk, encrypt_manifest, split_symmetric_key, sign_envelope, KEY_DIR};

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

// Key handling is delegated to the shared `crypto` crate.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
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

    let contents = tokio::fs::read_to_string(&path).await?;

    // Parse manifest to JSON if possible, else wrap raw
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };

    // ensure keypair
    let (pk_bytes, sk_bytes) = ensure_keypair_on_disk()?;

    // encrypt manifest
    let (ciphertext, nonce_bytes, sym, nonce) = encrypt_manifest(&manifest_json)?;

    // split symmetric key into shares (n=3, k=2)
    let n = 3usize;
    let k = 2usize;
    let shares_vec = split_symmetric_key(&sym, n, k);

    // store shares under ~/.beemesh/shares
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
    let shares_dir = home.join(KEY_DIR).join("shares");
    if !shares_dir.exists() {
        fs::create_dir_all(&shares_dir)?;
    }
    for (i, share) in shares_vec.iter().enumerate() {
        let fname = shares_dir.join(format!("share-{}.bin", i + 1));
        fs::write(&fname, share)?;
    }

    // build manifest envelope (contains encrypted manifest payload to store in DHT)
    let manifest_envelope = serde_json::json!({
        "payload": base64::engine::general_purpose::STANDARD.encode(&ciphertext),
        "nonce": base64::engine::general_purpose::STANDARD.encode(&nonce_bytes),
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() },
    });

    // sign manifest envelope
    let manifest_envelope_bytes = serde_json::to_vec(&manifest_envelope)?;
    let (manifest_sig_b64, manifest_pub_b64) = sign_envelope(&sk_bytes, &pk_bytes, &manifest_envelope_bytes)?;
    let mut manifest_envelope_signed = manifest_envelope;
    manifest_envelope_signed["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", manifest_sig_b64));
    manifest_envelope_signed["pubkey"] = serde_json::Value::String(manifest_pub_b64.clone());

    // build shares envelope (contains the base64 shares for peers)
    let shares_b64: Vec<String> = shares_vec.iter().map(|s| base64::engine::general_purpose::STANDARD.encode(s)).collect();
    let shares_payload = serde_json::json!({
        "shares": shares_b64,
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() }
    });
    let shares_envelope_bytes = serde_json::to_vec(&shares_payload)?;
    let (shares_sig_b64, shares_pub_b64) = sign_envelope(&sk_bytes, &pk_bytes, &shares_envelope_bytes)?;
    let mut shares_envelope_signed = shares_payload;
    shares_envelope_signed["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", shares_sig_b64));
    shares_envelope_signed["pubkey"] = serde_json::Value::String(shares_pub_b64);

    // wrapper message containing both envelopes
    let final_msg = serde_json::json!({
        "manifest_envelope": manifest_envelope_signed,
        "shares_envelope": shares_envelope_signed,
    });

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    let url = format!("{}/tenant/{}/apply", base.trim_end_matches('/'), tenant);

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(final_msg.to_string())
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    log::info!("API response: {}\n{}", status, body);

    if !status.is_success() {
        anyhow::bail!("apply failed: {}", status);
    }

    log::info!("Apply succeeded");

    Ok(())
}
