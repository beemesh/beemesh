use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
use protocol::machine::{build_envelope_signed, fb_envelope_extract_sig_pub};

#[test]
fn fb_helper_roundtrip() {
    ensure_pqc_init().expect("init");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("keypair");

    let payload = b"hello-fb".to_vec();
    let payload_type = "test";
    let nonce = "n-123";
    let ts = 42u64;
    let alg = "ml-dsa-65";

    // Build canonical bytes and sign using crypto helper
    let canonical =
        protocol::machine::build_envelope_canonical(&payload, payload_type, nonce, ts, alg, None);
    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("sign");

    // Build a signed flatbuffer envelope using the same format
    let fb = build_envelope_signed(
        &payload,
        payload_type,
        nonce,
        ts,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Use helper to extract and verify fields
    let (canon2, sig_bytes, pub_bytes, sig_field, pub_field) =
        fb_envelope_extract_sig_pub(&fb).expect("extract");

    // canonical bytes should match
    assert_eq!(canonical, canon2);
    // signature/pub decode to non-empty bytes
    assert!(!sig_bytes.is_empty());
    assert!(!pub_bytes.is_empty());
    assert!(sig_field.contains("ml-dsa-65"));
    assert_eq!(pub_field, pub_b64);
}
