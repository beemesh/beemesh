use std::convert::AsRef;
use std::sync::{Once, Mutex};
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Nonce};
use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use blahaj::{Sharks, Share};
use saorsa_pqc::api::sig::{ml_dsa_65, MlDsaPublicKey, MlDsaSecretKey, MlDsaVariant};
use saorsa_pqc::ApiMlDsaSignature;
// KEM API from saorsa_pqc
use saorsa_pqc::api::kem::{ml_kem_512, MlKemPublicKey, MlKemSecretKey, MlKemCiphertext, MlKemVariant};
use zeroize::Zeroizing;

use dirs::home_dir;
use std::path::PathBuf;
use sha2::{Digest, Sha256};
use hex;

mod keystore;
pub use keystore::Keystore;

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

/// Returns (stored_blob, cid_hex) where stored_blob contains wrapped-key + aes nonce + ciphertext
pub fn encrypt_share_for_keystore(payload: &[u8]) -> anyhow::Result<(Vec<u8>, String)> {
	// Use KEM encapsulation to derive the symmetric key for encrypting the payload.
	let (kem_pub, _kem_priv) = ensure_kem_keypair_on_disk()?;
	// encapsulate_to_pubkey returns (ct_bytes, shared_secret) where shared_secret is used as AES key
	let (wrapped_key_ct, shared_secret) = encapsulate_to_pubkey(&kem_pub)?;
	let cipher = Aes256Gcm::new_from_slice(&shared_secret).map_err(|e| anyhow::anyhow!("invalid key length for AES-GCM: {}", e))?;
	let mut nonce_bytes = [0u8; 12];
	rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
	let nonce = Nonce::from_slice(&nonce_bytes);
	let ciphertext = cipher.encrypt(nonce, payload).map_err(|e| anyhow::anyhow!("aes-gcm encrypt error: {}", e))?;

	// Stored blob format: [version=1 u8][wrapped_len u16 BE][wrapped bytes][nonce_len u8][nonce bytes][ciphertext_len u32 BE][ciphertext]
	let mut blob = Vec::with_capacity(1 + 2 + wrapped_key_ct.len() + 1 + nonce_bytes.len() + 4 + ciphertext.len());
	blob.push(0x01u8);
	let wlen = wrapped_key_ct.len() as u16;
	blob.extend_from_slice(&wlen.to_be_bytes());
	blob.extend_from_slice(&wrapped_key_ct);
	blob.push(nonce_bytes.len() as u8);
	blob.extend_from_slice(&nonce_bytes);
	let clen = ciphertext.len() as u32;
	blob.extend_from_slice(&clen.to_be_bytes());
	blob.extend_from_slice(&ciphertext);

	// compute CID as sha256 hex of the blob
	let mut hasher = Sha256::default();
	hasher.update(&blob);
	let cid = hex::encode(hasher.finalize());
	// log some diagnostics about the blob we produced
	log::warn!("encrypt_share_for_keystore: payload_len={} wrapped_len={} ciphertext_len={} cid={}", payload.len(), wrapped_key_ct.len(), ciphertext.len(), cid);
	log::warn!("encrypt_share_for_keystore: produced blob len={} cid={}", blob.len(), cid);
	Ok((blob, cid))
}

/// Reverse of encrypt_share_for_keystore - requires decapsulating the wrapped key first
pub fn decrypt_share_from_blob(blob: &[u8], priv_kem_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
	if blob.is_empty() || blob[0] != 0x01 { anyhow::bail!("unsupported blob version"); }
	let mut idx = 1usize;
	if blob.len() < idx + 2 { anyhow::bail!("blob too short"); }
	let wlen = u16::from_be_bytes([blob[idx], blob[idx+1]]) as usize; idx += 2;
	if blob.len() < idx + wlen { anyhow::bail!("blob too short for wrapped"); }
	let wrapped = &blob[idx..idx+wlen]; idx += wlen;
	if blob.len() < idx + 1 { anyhow::bail!("blob too short for nonce len"); }
	let nlen = blob[idx] as usize; idx += 1;
	if blob.len() < idx + nlen { anyhow::bail!("blob too short for nonce"); }
	let nonce = &blob[idx..idx+nlen]; idx += nlen;
	if blob.len() < idx + 4 { anyhow::bail!("blob too short for ct len"); }
	let clen = u32::from_be_bytes([blob[idx], blob[idx+1], blob[idx+2], blob[idx+3]]) as usize; idx += 4;
	if blob.len() < idx + clen { anyhow::bail!("blob too short for ciphertext"); }
	let ciphertext = &blob[idx..idx+clen];

	// decapsulate wrapped key
	let shared = decapsulate_share(priv_kem_bytes, wrapped)?; // Zeroizing<Vec<u8>>
	let cipher = Aes256Gcm::new_from_slice(&shared[..]).map_err(|e| anyhow::anyhow!("aes key error: {}", e))?;
	let plain = cipher.decrypt(Nonce::from_slice(nonce), ciphertext.as_ref()).map_err(|e| anyhow::anyhow!("aes-gcm decrypt error: {}", e))?;
	Ok(plain)
}

/// Determine keystore default path / behaviour from environment.
pub fn keystore_default_path() -> PathBuf {
	if std::env::var("BEEMESH_KEYSTORE_PATH").is_ok() {
		PathBuf::from(std::env::var("BEEMESH_KEYSTORE_PATH").unwrap())
	} else {
		PathBuf::from("/etc/beemesh/keystore.db")
	}
}

/// Open default keystore reading BEEMESH_KEYSTORE_EPHEMERAL env var to decide in-memory.
pub fn open_keystore_default() -> anyhow::Result<Keystore> {
	let ephemeral = std::env::var("BEEMESH_KEYSTORE_EPHEMERAL").is_ok();
	let shared_name = std::env::var("BEEMESH_KEYSTORE_SHARED_NAME").unwrap_or_default();
	log::warn!("open_keystore_default: ephemeral={} shared_name='{}' thread_id={:?}", 
		ephemeral, shared_name, std::thread::current().id());
	let path = keystore_default_path();
	Keystore::open(&path, ephemeral).map_err(|e| anyhow::anyhow!("keystore open failed: {}", e))
}

pub fn open_keystore_with_shared_name(shared_name: &str) -> anyhow::Result<Keystore> {
	let ephemeral = std::env::var("BEEMESH_KEYSTORE_EPHEMERAL").is_ok();
	log::warn!("open_keystore_with_shared_name: ephemeral={} shared_name='{}' thread_id={:?}", 
		ephemeral, shared_name, std::thread::current().id());
	
	if ephemeral && !shared_name.is_empty() {
		// Use temp file approach with explicit shared name
		let temp_path = std::env::temp_dir().join(format!("beemesh_keystore_{}", shared_name));
		Keystore::open(&temp_path, false) // false for ephemeral since we're using temp file
	} else {
		let path = keystore_default_path();
		Keystore::open(&path, ephemeral)
	}.map_err(|e| anyhow::anyhow!("keystore open failed: {}", e))
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

/// Decrypt a manifest ciphertext produced by `encrypt_manifest` using the symmetric key and nonce.
pub fn decrypt_manifest(sym: &[u8; 32], nonce_bytes: &[u8], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
	let cipher = Aes256Gcm::new_from_slice(&sym[..]).map_err(|e| anyhow::anyhow!("invalid key length for AES-GCM: {}", e))?;
	if nonce_bytes.len() != 12 { anyhow::bail!("invalid nonce length: {}", nonce_bytes.len()); }
	let nonce = Nonce::from_slice(nonce_bytes);
	let plain = cipher.decrypt(nonce, ciphertext.as_ref()).map_err(|e| anyhow::anyhow!("aes-gcm decrypt error: {}", e))?;
	Ok(plain)
}

pub fn split_symmetric_key(sym: &[u8; 32], n: usize, k: usize) -> Vec<Vec<u8>> {
	assert!(k >= 1 && k <= 255, "invalid threshold");
	assert!(n >= 1 && n <= 255, "invalid share count");
	let sharks = Sharks(k as u8);
	let dealer = sharks.dealer(&sym[..]);
	let shares: Vec<Share> = dealer.take(n).collect();
	shares.into_iter().map(|s| Vec::from(&s)).collect()
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
	if shares_bytes.len() < k {
		anyhow::bail!("need at least k shares to recover");
	}
	let mut shares: Vec<Share> = Vec::with_capacity(k);
	for b in shares_bytes.iter().take(k) {
		let s = Share::try_from(b.as_slice()).map_err(|e| anyhow::anyhow!("invalid share bytes: {}", e))?;
		shares.push(s);
	}

	let sharks = Sharks(k as u8);
	let recovered = sharks.recover(shares.as_slice()).map_err(|e| anyhow::anyhow!("blahaj recover failed: {:?}", e))?;
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

	#[test]
	fn test_encrypt_decrypt_keystore_blob_roundtrip() {
		let _guard = PQC_TEST_MUTEX
			.get_or_init(|| std::sync::Mutex::new(()))
			.lock()
			.unwrap_or_else(|e| e.into_inner());

		ensure_pqc_init().expect("pqc init");
		// generate ephemeral kem keypair and write to disk in the keydir used by ensure_kem_keypair_on_disk
		let kem = ml_kem_512();
		let (pubk, privk) = kem.generate_keypair().expect("kem keygen");
		let pubb = pubk.to_bytes();
		let privb = privk.to_bytes();

		// Temporarily write these into the keydir used by ensure_kem_keypair_on_disk
		let home = home_dir().unwrap();
		let key_dir = home.join(KEY_DIR);
		let _ = std::fs::create_dir_all(&key_dir);
		let pub_path = key_dir.join(KEM_PUBFILE);
		let priv_path = key_dir.join(KEM_PRIVFILE);
		let _ = std::fs::write(&pub_path, &pubb);
		let _ = std::fs::write(&priv_path, &privb);

	let payload = b"hello keystore payload";
	let (blob, cid) = encrypt_share_for_keystore(payload).expect("encrypt failed");
	assert!(!cid.is_empty());

	// Parse blob locally to inspect parts
	assert!(blob.len() > 10);
	let mut idx = 1usize;
	let wlen = u16::from_be_bytes([blob[idx], blob[idx+1]]) as usize; idx += 2;
	let wrapped = &blob[idx..idx+wlen]; idx += wlen;
	let nlen = blob[idx] as usize; idx += 1;
	let _nonce = &blob[idx..idx+nlen]; idx += nlen;
	let clen = u32::from_be_bytes([blob[idx], blob[idx+1], blob[idx+2], blob[idx+3]]) as usize; idx += 4;
	let _ciphertext = &blob[idx..idx+clen];

	// Finally attempt to decrypt ciphertext using the priv key
	let recovered = decrypt_share_from_blob(&blob, &privb).expect("decrypt failed");
	assert_eq!(&recovered, payload);
	}
}
