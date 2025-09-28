use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose, Engine as _};
use clap::{Parser, Subcommand};
use dirs::home_dir;
use reqwest;
use saorsa_pqc::api::sig::{ml_dsa_65, MlDsaPublicKey, MlDsaSecretKey, MlDsaVariant};
use sharks::{Sharks, Share};
use rand::RngCore;
use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::fs;
use std::path::PathBuf;

const KEY_DIR: &str = ".beemesh";
const PUBKEY_FILE: &str = "pubkey.bin";
const PRIVKEY_FILE: &str = "privkey.bin";

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

fn ensure_keypair() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    // returns (pubkey_bytes, privkey_bytes)
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
    let dir = home.join(KEY_DIR);
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }

    let pubpath = dir.join(PUBKEY_FILE);
    let privpath = dir.join(PRIVKEY_FILE);
    if pubpath.exists() && privpath.exists() {
        let pk = fs::read(&pubpath)?;
        let sk = fs::read(&privpath)?;
        return Ok((pk, sk));
    }

    // generate new ML-DSA-65 keypair
    let dsa = ml_dsa_65();
    let (pubk, privk) = dsa.generate_keypair()?;
    let pk_bytes = pubk.to_bytes();
    let sk_bytes = privk.to_bytes();

    fs::write(&pubpath, &pk_bytes)?;
    fs::write(&privpath, &sk_bytes)?;

    Ok((pk_bytes, sk_bytes))
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

    let contents = tokio::fs::read_to_string(&path).await?;

    // Parse manifest to JSON if possible, else wrap raw
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };

    // ensure keypair
    let (pk_bytes, sk_bytes) = ensure_keypair()?;

    // generate symmetric key
    let mut sym = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut sym);

    // encrypt manifest_json as bytes using AES-256-GCM
    let cipher = Aes256Gcm::new_from_slice(&sym).map_err(|e| anyhow::anyhow!("invalid key length for AES-GCM: {}", e))?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = serde_json::to_vec(&manifest_json)?;
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|e| anyhow::anyhow!("aes-gcm encrypt error: {}", e))?;

    // split symmetric key into shares (n=3, k=2) using sharks
    let n = 3usize;
    let k = 2usize;
    let sharks = Sharks(k as u8);
    // dealer is an iterator of Share; take `n` shares
    let dealer = sharks.dealer(&sym);
    let shares: Vec<Share> = dealer.take(n).collect();
    let mut shares_vec: Vec<Vec<u8>> = Vec::new();
    for s in &shares {
        shares_vec.push(Vec::from(s));
    }

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

    // build envelope
    let envelope = serde_json::json!({
        "payload": general_purpose::STANDARD.encode(&ciphertext),
        "nonce": general_purpose::STANDARD.encode(&nonce_bytes),
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() },
    });

    // sign envelope using saorsa-pqc MlDsa (ML-DSA-65)
    let dsa = ml_dsa_65();
    let privk = MlDsaSecretKey::from_bytes(MlDsaVariant::MlDsa65, &sk_bytes)?;
    let pubk = MlDsaPublicKey::from_bytes(MlDsaVariant::MlDsa65, &pk_bytes)?;
    let envelope_bytes = serde_json::to_vec(&envelope)?;
    let signature = dsa.sign(&privk, &envelope_bytes)?;
    let sig_b64 = general_purpose::STANDARD.encode(&signature.to_bytes());

    // final message
    let mut final_msg = envelope;
    final_msg["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", sig_b64));
    final_msg["pubkey"] = serde_json::Value::String(general_purpose::STANDARD.encode(&pubk.to_bytes()));

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
    println!("API response: {}\n{}", status, body);

    if !status.is_success() {
        anyhow::bail!("apply failed: {}", status);
    }

    println!("Apply succeeded");

    Ok(())
}
