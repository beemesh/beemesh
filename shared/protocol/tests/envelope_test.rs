use anyhow::Context;
use base64::Engine as _;
use base64::engine::general_purpose;
use flatbuffers::FlatBufferBuilder;

use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope, verify_envelope};
use protocol::machine::{EnvelopeArgs, FbEnvelope, finish_envelope_buffer};

#[test]
fn flatbuffer_envelope_sign_and_verify() -> anyhow::Result<()> {
    // Initialize PQC runtime
    ensure_pqc_init()?;

    // Generate ephemeral keypair (pub, priv)
    let (pub_bytes, priv_bytes) = ensure_keypair_ephemeral()?;

    // Sample inner payload
    let payload_bytes = b"hello inner payload";

    // Build a canonical envelope with empty sig/pubkey (these are the bytes we sign)
    let mut fbb = FlatBufferBuilder::with_capacity(256);
    let payload_vec = fbb.create_vector(payload_bytes);
    let payload_type = fbb.create_string("test.payload.v1");
    let nonce = fbb.create_string("nonce-abc-123");
    let alg = fbb.create_string("ml-dsa-65");
    let sig = fbb.create_string("");
    let pubkey = fbb.create_string("");
    let peer_id = fbb.create_string("");

    let mut args = EnvelopeArgs::default();
    args.payload = Some(payload_vec);
    args.payload_type = Some(payload_type);
    args.nonce = Some(nonce);
    args.ts = 123456789_u64;
    args.alg = Some(alg);
    args.sig = Some(sig);
    args.pubkey = Some(pubkey);
    args.peer_id = Some(peer_id);

    let env_off = FbEnvelope::create(&mut fbb, &args);
    finish_envelope_buffer(&mut fbb, env_off);
    let canonical_bytes = fbb.finished_data();

    // Sign the canonical bytes
    let (sig_b64, pub_b64) = sign_envelope(&priv_bytes, &pub_bytes, canonical_bytes)?;

    // Now build an envelope with sig and pubkey populated
    let mut fbb2 = FlatBufferBuilder::with_capacity(256);
    let payload_vec2 = fbb2.create_vector(payload_bytes);
    let payload_type2 = fbb2.create_string("test.payload.v1");
    let nonce2 = fbb2.create_string("nonce-abc-123");
    let alg2 = fbb2.create_string("ml-dsa-65");
    let sig_str = fbb2.create_string(&format!("ml-dsa-65:{}", sig_b64));
    let pubkey_str = fbb2.create_string(&pub_b64);
    let peer_id2 = fbb2.create_string("");

    let mut args2 = EnvelopeArgs::default();
    args2.payload = Some(payload_vec2);
    args2.payload_type = Some(payload_type2);
    args2.nonce = Some(nonce2);
    args2.ts = 123456789_u64;
    args2.alg = Some(alg2);
    args2.sig = Some(sig_str);
    args2.pubkey = Some(pubkey_str);
    args2.peer_id = Some(peer_id2);

    let env_off2 = FbEnvelope::create(&mut fbb2, &args2);
    finish_envelope_buffer(&mut fbb2, env_off2);
    let _signed_buf = fbb2.finished_data();

    // Decode signature bytes from base64
    // Note: sign_envelope returned (sig_b64, pub_b64) where sig_b64 is the base64 of raw sig bytes
    let sig_bytes = general_purpose::STANDARD
        .decode(&sig_b64)
        .context("decode sig b64")?;

    // Verify signature over the canonical bytes using public key bytes
    verify_envelope(&pub_bytes, canonical_bytes, &sig_bytes)?;

    // As a negative test, tamper a byte in canonical bytes and verify fails
    let mut tampered = canonical_bytes.to_vec();
    if !tampered.is_empty() {
        tampered[0] ^= 0xff;
    }
    let res = verify_envelope(&pub_bytes, &tampered, &sig_bytes);
    assert!(res.is_err(), "tampered buffer should not verify");

    Ok(())
}
