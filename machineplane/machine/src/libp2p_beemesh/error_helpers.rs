// Common error constructors for libp2p-beemesh.
use anyhow::{anyhow, Error};
use std::fmt::Display;

pub fn control_sender_unavailable() -> Error {
    anyhow!("control sender not available")
}

pub fn wrap_send_error<E: Display>(op: &str, e: E) -> Error {
    anyhow!("failed to send {} control message: {}", op, e)
}

pub fn insufficient_key_shares(have: usize, need: usize) -> Error {
    anyhow!(
        "insufficient key shares after distributed retrieval: have {} need {}",
        have,
        need
    )
}

pub fn not_enough_local_key_shares(more_needed: usize) -> Error {
    anyhow!("not enough local key shares - need {} more", more_needed)
}
