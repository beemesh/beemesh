//! Centralized envelope validation utilities to eliminate duplication
//!
//! This module provides reusable envelope validation logic that was previously
//! duplicated across multiple behavior modules in the machine crate.

use anyhow::{Result, anyhow};
use log::warn;
use super::logging::CryptoLogger;

/// Envelope validation error types
#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    #[error("Envelope verification failed: {0}")]
    VerificationFailed(String),
    #[error("Unsigned envelope rejected: {0}")]
    UnsignedRejected(String),
    #[error("Invalid envelope format: {0}")]
    InvalidFormat(String),
    #[error("Signature validation failed: {0}")]
    SignatureFailed(String),
}

/// Direction of envelope validation (for logging)
#[derive(Debug, Clone, Copy)]
pub enum ValidationDirection {
    Inbound,
    Outbound,
}

impl std::fmt::Display for ValidationDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationDirection::Inbound => write!(f, "inbound"),
            ValidationDirection::Outbound => write!(f, "outbound"),
        }
    }
}

/// Centralized envelope validator
pub struct EnvelopeValidator;

impl EnvelopeValidator {
    /// Validate envelope and return payload, or fallback to original request if unsigned messages are allowed
    pub fn validate_or_fallback(
        request: &[u8],
        protocol: &str,
        direction: ValidationDirection,
    ) -> Result<Vec<u8>, EnvelopeError> {
        // First try to parse as JSON for verification
        match serde_json::from_slice::<serde_json::Value>(request) {
            Ok(envelope_val) => {
                // Try to verify the envelope
                match verify_envelope_and_check_nonce(&envelope_val) {
                    Ok((payload_bytes, _pub, _sig)) => {
                        CryptoLogger::log_signature_verification(true, None);
                        Ok(payload_bytes)
                    }
                    Err(e) => {
                        if Self::require_signed_messages() {
                            let error_msg = format!(
                                "rejecting unsigned/invalid {} request in {}: {:?}",
                                direction, protocol, e
                            );
                            warn!("{}", error_msg);
                            Err(EnvelopeError::UnsignedRejected(error_msg))
                        } else {
                            // Fallback to original request if signing is not required
                            warn!(
                                "accepting unsigned {} request in {} (signing not required)",
                                direction, protocol
                            );
                            Ok(request.to_vec())
                        }
                    }
                }
            }
            Err(_) => {
                // Not JSON, treat as raw request
                if Self::require_signed_messages() {
                    let error_msg = format!(
                        "rejecting non-JSON {} request in {} (signing required)",
                        direction, protocol
                    );
                    warn!("{}", error_msg);
                    Err(EnvelopeError::InvalidFormat(error_msg))
                } else {
                    Ok(request.to_vec())
                }
            }
        }
    }

    /// Validate envelope and reject if unsigned (strict validation)
    pub fn validate_strict(
        request: &[u8],
        protocol: &str,
        direction: ValidationDirection,
    ) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), EnvelopeError> {
        match serde_json::from_slice::<serde_json::Value>(request) {
            Ok(envelope_val) => match verify_envelope_and_check_nonce(&envelope_val) {
                Ok((payload_bytes, pub_key, signature)) => {
                    CryptoLogger::log_signature_verification(true, None);
                    Ok((payload_bytes, pub_key, signature))
                }
                Err(e) => {
                    let error_msg = format!(
                        "envelope verification failed for {} request in {}: {:?}",
                        direction, protocol, e
                    );
                    warn!("{}", error_msg);
                    CryptoLogger::log_signature_verification(false, None);
                    Err(EnvelopeError::VerificationFailed(error_msg))
                }
            },
            Err(e) => {
                let error_msg = format!(
                    "invalid JSON format for {} request in {}: {:?}",
                    direction, protocol, e
                );
                warn!("{}", error_msg);
                Err(EnvelopeError::InvalidFormat(error_msg))
            }
        }
    }

    /// Validate flatbuffer envelope
    pub fn validate_flatbuffer_envelope(
        _request: &[u8],
        _protocol: &str,
        _direction: ValidationDirection,
        _timeout: std::time::Duration,
    ) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), EnvelopeError> {
        // This function needs to be imported from the machine crate
        // For now, return an error indicating this needs to be implemented
        Err(EnvelopeError::VerificationFailed(
            "Flatbuffer envelope verification not yet implemented in crypto crate".to_string(),
        ))
    }

    /// Check if signed messages are required based on environment variable
    pub fn require_signed_messages() -> bool {
        true
    }

    /// Validate envelope with custom error handler
    pub fn validate_with_error_handler<F, R>(
        request: &[u8],
        protocol: &str,
        direction: ValidationDirection,
        error_handler: F,
    ) -> Option<Vec<u8>>
    where
        F: FnOnce(&str) -> R,
    {
        match Self::validate_or_fallback(request, protocol, direction) {
            Ok(payload) => Some(payload),
            Err(e) => {
                error_handler(&e.to_string());
                None
            }
        }
    }
}

/// Helper function to verify envelope and check nonce (wrapper for backwards compatibility)
pub fn verify_envelope_and_check_nonce(
    _envelope_val: &serde_json::Value,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    // Use the existing security module's function
    // Placeholder implementation - this should be moved from machine crate
    Err(anyhow!(
        "verify_envelope_and_check_nonce not yet moved to crypto crate"
    ))
}

/// Convenient type alias for envelope validation results
pub type ValidationResult<T> = Result<T, EnvelopeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_require_signed_messages() {
        assert!(EnvelopeValidator::require_signed_messages());
    }

    #[test]
    fn test_validation_direction_display() {
        assert_eq!(format!("{}", ValidationDirection::Inbound), "inbound");
        assert_eq!(format!("{}", ValidationDirection::Outbound), "outbound");
    }

    #[test]
    fn test_envelope_error_display() {
        let error = EnvelopeError::VerificationFailed("test error".to_string());
        assert!(error.to_string().contains("test error"));

        let error = EnvelopeError::UnsignedRejected("unsigned".to_string());
        assert!(error.to_string().contains("unsigned"));

        let error = EnvelopeError::InvalidFormat("format".to_string());
        assert!(error.to_string().contains("format"));

        let error = EnvelopeError::SignatureFailed("signature".to_string());
        assert!(error.to_string().contains("signature"));
    }

    #[test]
    fn test_validate_with_error_handler() {
        let invalid_json = b"invalid json";
        let mut error_message = String::new();

        let result = EnvelopeValidator::validate_with_error_handler(
            invalid_json,
            "test_protocol",
            ValidationDirection::Inbound,
            |err| error_message = err.to_string(),
        );

        // Should return None for invalid input when signing is not required
        // The actual behavior depends on BEEMESH_REQUIRE_SIGNED_MESSAGES
        if EnvelopeValidator::require_signed_messages() {
            assert!(result.is_none());
            assert!(!error_message.is_empty());
        }
    }
}
