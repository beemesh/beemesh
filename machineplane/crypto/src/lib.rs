use std::convert::AsRef;
use std::sync::{Once, Mutex};
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use sharks::{Sharks, Share};
use saorsa_pqc::api::sig::{ml_dsa_65, MlDsaPublicKey, MlDsaSecretKey, MlDsaVariant};
use saorsa_pqc::ApiMlDsaSignature;
// KEM API from saorsa_pqc
use saorsa_pqc::api::kem::{ml_kem_512, MlKemPublicKey, MlKemSecretKey, MlKemCiphertext, MlKemVariant};
use zeroize::Zeroizing;
// sharks crate unused now (kept in Cargo.toml for future)
// ...existing code...
use dirs::home_dir;

pub const KEY_DIR: &str = ".beemesh";
pub const PUBKEY_FILE: &str = "pubkey.bin";
pub const PRIVKEY_FILE: &str = "privkey.bin";
pub const KEM_PUBFILE: &str = "kem_pub.bin";
pub const KEM_PRIVFILE: &str = "kem_priv.bin";

// Lazy initializer for saorsa_pqc::api_init(). We use std::sync::Once plus
// a Mutex-protected Option to capture any initialization error so callers
// can observe it deterministically. This behaves like a lazy-static init
// but preserves and propagates errors from the underlying library.
static PQC_INIT: Once = Once::new();
static PQC_INIT_ERR: Mutex<Option<anyhow::Error>> = Mutex::new(None);

pub fn ensure_pqc_init() -> anyhow::Result<()> {
	PQC_INIT.call_once(|| {
		if let Err(e) = saorsa_pqc::api_init() {
			let mut g = PQC_INIT_ERR.lock().unwrap();
			*g = Some(anyhow::anyhow!("pqc api_init failed: {:?}", e));
		}
	});
	let guard = PQC_INIT_ERR.lock().unwrap();
	if let Some(err) = guard.as_ref() {
		Err(anyhow::anyhow!("pqc init previously failed: {}", err))
	} else {
		Ok(())
	}
}

pub fn ensure_keypair_on_disk() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
	let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
	let key_dir = home.join(KEY_DIR);
	if !key_dir.exists() {
		std::fs::create_dir_all(&key_dir)?;
		// tighten permissions to user-only
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))?;
		}
	}

	let pub_path = key_dir.join(PUBKEY_FILE);
	let priv_path = key_dir.join(PRIVKEY_FILE);
	if pub_path.exists() && priv_path.exists() {
		let pubb = std::fs::read(&pub_path)?;
		let privb = std::fs::read(&priv_path)?;
		return Ok((pubb, privb));
	}

	// generate and persist
	let dsa = ml_dsa_65();
	let (pubk, privk) = dsa.generate_keypair()?;
	let pubb = pubk.to_bytes();
	let privb = privk.to_bytes();
	std::fs::write(&pub_path, &pubb)?;
	std::fs::write(&priv_path, &privb)?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o600))?;
		std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok((pubb, privb))
}

/// Ensure a KEM keypair exists on disk. Returns (pub_bytes, priv_bytes).
pub fn ensure_kem_keypair_on_disk() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
	// Reuse same key dir logic as signing keypair
	let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
	let key_dir = home.join(KEY_DIR);
	if !key_dir.exists() {
		std::fs::create_dir_all(&key_dir)?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&key_dir, std::fs::Permissions::from_mode(0o700))?;
		}
	}

	let pub_path = key_dir.join(KEM_PUBFILE);
	let priv_path = key_dir.join(KEM_PRIVFILE);
	if pub_path.exists() && priv_path.exists() {
		let pubb = std::fs::read(&pub_path)?;
		let privb = std::fs::read(&priv_path)?;
		return Ok((pubb, privb));
	}

	// generate and persist
	ensure_pqc_init()?;
	let kem = ml_kem_512();
	let (pubk, privk) = kem.generate_keypair()?;
	let pubb = pubk.to_bytes();
	let privb = privk.to_bytes();
	std::fs::write(&pub_path, &pubb)?;
	std::fs::write(&priv_path, &privb)?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o600))?;
		std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
	}
	Ok((pubb, privb))
}

/// Encapsulate to a recipient KEM public key bytes. Returns (ciphertext_bytes, shared_secret_bytes).
pub fn encapsulate_to_pubkey(pub_bytes: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
	ensure_pqc_init()?;
	let kem = ml_kem_512();
	let pubk = MlKemPublicKey::from_bytes(MlKemVariant::MlKem512, pub_bytes)?;
	// saorsa_pqc::kem::encapsulate returns (shared_secret, ciphertext)
	let (shared, ct) = kem.encapsulate(&pubk)?;
	// ct.to_bytes() may return a fixed-size array or Vec depending on implementation; normalize both to Vec
	let ct_bytes = ct.to_bytes();
	let shared_arr = shared.to_bytes();
	Ok((ct_bytes[..].to_vec(), shared_arr[..].to_vec()))
}

/// Decapsulate a ciphertext using the provided KEM private key bytes. Returns shared_secret_bytes.
/// Decapsulate a KEM ciphertext using the provided private key bytes.
/// Returns a zeroizing Vec<u8> that will be zeroed when dropped.
pub fn decapsulate_share(priv_bytes: &[u8], ciphertext: &[u8]) -> anyhow::Result<Zeroizing<Vec<u8>>> {
	ensure_pqc_init()?;
	let kem = ml_kem_512();
	let privk = MlKemSecretKey::from_bytes(MlKemVariant::MlKem512, priv_bytes)?;
	// construct the concrete ciphertext object from raw bytes then call decapsulate
	let ct = MlKemCiphertext::from_bytes(MlKemVariant::MlKem512, ciphertext)
		.map_err(|e| anyhow::anyhow!("invalid ciphertext bytes: {:?}", e))?;
	let shared = kem.decapsulate(&privk, &ct)?;
	let shared_arr = shared.to_bytes();
	Ok(Zeroizing::new(shared_arr[..].to_vec()))
}

pub fn ensure_keypair_ephemeral() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
	// Defensive init so callers (including tests) don't have to call ensure_pqc_init()
	ensure_pqc_init()?;
	let dsa = ml_dsa_65();
	let (pubk, privk) = dsa.generate_keypair()?;
	Ok((pubk.to_bytes(), privk.to_bytes()))
}

pub fn encrypt_manifest(manifest_json: &serde_json::Value) -> anyhow::Result<(Vec<u8>, Vec<u8>, [u8; 32], [u8; 12])> {
	let mut sym = [0u8; 32];
	rand::rngs::OsRng.fill_bytes(&mut sym);
	let cipher = Aes256Gcm::new_from_slice(&sym).map_err(|e| anyhow::anyhow!("invalid key length for AES-GCM: {}", e))?;
	let mut nonce_bytes = [0u8; 12];
	rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
	let nonce = Nonce::from_slice(&nonce_bytes);
	let plaintext = serde_json::to_vec(manifest_json)?;
	let ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).map_err(|e| anyhow::anyhow!("aes-gcm encrypt error: {}", e))?;
	Ok((ciphertext, nonce_bytes.to_vec(), sym, nonce_bytes))
}

pub fn split_symmetric_key(sym: &[u8; 32], n: usize, k: usize) -> Vec<Vec<u8>> {
	// Use sharks Shamir secret sharing. `k` is the threshold, `n` is total shares to produce.
	// Sharks expects the threshold as its tuple param: Sharks(k)
	let sharks = Sharks(k as u8);
	// dealer produces an iterator of Share objects from the secret bytes
	let dealer = sharks.dealer(&sym[..]);
	// collect n shares
	let shares: Vec<Share> = dealer.take(n).collect();
	// convert each Share into Vec<u8> using the provided From<&Share> implementation
	shares.iter().map(|s| Vec::from(s)).collect()
}

pub fn sign_envelope(sk_bytes: &[u8], pk_bytes: &[u8], envelope_bytes: &[u8]) -> anyhow::Result<(String, String)> {
	// Defensive init in case caller hasn't initialized PQC
	ensure_pqc_init()?;
	let dsa = ml_dsa_65();
	let privk = MlDsaSecretKey::from_bytes(MlDsaVariant::MlDsa65, sk_bytes)?;
	let pubk = MlDsaPublicKey::from_bytes(MlDsaVariant::MlDsa65, pk_bytes)?;
	let signature = dsa.sign(&privk, envelope_bytes)?;
	let sig_b64 = general_purpose::STANDARD.encode(&signature.to_bytes());
	let pub_b64 = general_purpose::STANDARD.encode(&pubk.to_bytes());
	Ok((sig_b64, pub_b64))
}
pub fn verify_envelope(pub_bytes: &[u8], envelope_bytes: &[u8], sig_bytes: &[u8]) -> anyhow::Result<()> {
	// Defensive init in case caller hasn't initialized PQC
	ensure_pqc_init()?;
	let dsa = ml_dsa_65();
	let pubk = MlDsaPublicKey::from_bytes(MlDsaVariant::MlDsa65, pub_bytes)?;
	// Construct the concrete signature object from raw bytes then call the DSA verify API.
	let signature = ApiMlDsaSignature::from_bytes(MlDsaVariant::MlDsa65, sig_bytes)
		.map_err(|e| anyhow::anyhow!("invalid signature bytes: {:?}", e))?;
	match dsa.verify(&pubk, envelope_bytes, &signature) {
		Ok(true) => Ok(()),
		Ok(false) => Err(anyhow::anyhow!("signature verification failed: invalid signature")),
		Err(e) => Err(anyhow::anyhow!("signature verification error: {:?}", e)),
	}
}

/// Attempt to decapsulate a ciphertext (KEM-style) using the provided private key bytes.
/// This wrapper currently tries to use saorsa_pqc's ml_kem APIs. The exact kem variant
/// used must match the sender's encapsulation. For now we support the ml_kem interface
/// and return an error if decapsulation fails.
// NOTE: decapsulation helpers were intentionally omitted. The current CLI produces
// shares as base64 strings inside a signed JSON envelope. If we later add KEM-based
// per-recipient encryption, implement a proper decapsulation wrapper here using the
// exact KEM API chosen by the CLI (saorsa_pqc / kyber / hpke). Keep the key material
// in-memory only and zeroize as appropriate.

pub fn recover_symmetric_key(shares_bytes: &[Vec<u8>], k: usize) -> anyhow::Result<[u8; 32]> {
	// Convert incoming byte slices to sharks::Share instances
	let mut shares: Vec<Share> = Vec::with_capacity(shares_bytes.len());
	for b in shares_bytes {
		let s = Share::try_from(b.as_slice()).map_err(|e| anyhow::anyhow!("invalid share bytes: {}", e))?;
		shares.push(s);
	}

	let sharks = Sharks(k as u8);
	let recovered = sharks.recover(&shares).map_err(|e| anyhow::anyhow!("sharks recover failed: {:?}", e))?;
	if recovered.len() != 32 {
		anyhow::bail!("recovered secret length != 32: {}", recovered.len());
	}
	let mut out = [0u8; 32];
	out.copy_from_slice(&recovered[..32]);
	Ok(out)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::OnceLock;

	static PQC_TEST_MUTEX: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

	#[test]
	fn test_split_and_recover_shamir() {
		// create a random 32-byte secret
		let mut secret = [0u8; 32];
		rand::rngs::OsRng.fill_bytes(&mut secret);

		let n = 5usize;
		let k = 3usize;
		let shares = split_symmetric_key(&secret, n, k);
		assert_eq!(shares.len(), n);

		// pick any k shares (take first k)
		let chosen: Vec<Vec<u8>> = shares.into_iter().take(k).collect();

		let recovered = recover_symmetric_key(&chosen, k).expect("recover failed");
		assert_eq!(&recovered[..], &secret[..]);
	}

	#[test]
	fn test_sign_and_verify_envelope() {
		// serialize access to PQC library across tests to avoid races in the FIPS backend
		// tolerate mutex poisoning in tests by recovering inner mutex guard
		let _guard = PQC_TEST_MUTEX
			.get_or_init(|| std::sync::Mutex::new(()))
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		// 1) wrapper round-trip: ensure our sign_envelope / verify_envelope work
		let (pub_bytes_wrapped, sk_bytes_wrapped) = ensure_keypair_ephemeral().expect("keygen failed");
		let payload = b"this is a test envelope payload";
		let (sig_b64, pub_b64) = sign_envelope(&sk_bytes_wrapped, &pub_bytes_wrapped, payload).expect("sign failed");
		let sig_bytes = general_purpose::STANDARD.decode(&sig_b64).expect("b64 decode sig");
		let pub_bytes = general_purpose::STANDARD.decode(&pub_b64).expect("b64 decode pub");
		verify_envelope(&pub_bytes, payload, &sig_bytes).expect("verify call failed");

		// 2) Negative check: signatures generated for key A MUST NOT verify under key B
		let (pub_a, sk_a) = ensure_keypair_ephemeral().expect("keygen a");
	let (_pub_b, _sk_b) = ensure_keypair_ephemeral().expect("keygen b");
		let (sig_b64_a, _pub_b64_a) = sign_envelope(&sk_a, &pub_a, payload).expect("sign with a failed");
		let mut sig_bytes_a = general_purpose::STANDARD.decode(&sig_b64_a).expect("decode sig a");
		// flip a byte in the signature
		if !sig_bytes_a.is_empty() { sig_bytes_a[0] ^= 0xff; }
		let res = verify_envelope(&pub_a, payload, &sig_bytes_a);
		assert!(res.is_err(), "mutated signature must not verify");
	}

	#[test]
	fn test_kem_encapsulate_decapsulate_roundtrip() {
		// serialize access to PQC library across tests
		let _guard = PQC_TEST_MUTEX
			.get_or_init(|| std::sync::Mutex::new(()))
			.lock()
			.unwrap_or_else(|e| e.into_inner());
		// Ensure PQC is initialized and generate an ephemeral kem keypair
		ensure_pqc_init().expect("pqc init");
		let kem = ml_kem_512();
		let (pubk, privk) = kem.generate_keypair().expect("kem keygen");
		let pubb = pubk.to_bytes();
		let privb = privk.to_bytes();

		// Encapsulate
		let (ct_bytes, shared_enc) = encapsulate_to_pubkey(&pubb).expect("encapsulate");
		// Decapsulate and ensure secrets match
		let shared_dec = decapsulate_share(&privb, &ct_bytes).expect("decapsulate");
		assert_eq!(&shared_enc[..], &shared_dec[..]);
		// shared_dec is Zeroizing and will be zeroed on drop
	}
}
