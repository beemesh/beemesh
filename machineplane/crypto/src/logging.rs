//! Structured logging utilities for consistent log formatting across the codebase
//!
//! This module provides macros and utilities to eliminate duplicated logging patterns
//! and ensure consistent log formatting throughout the application.

/// Macro for libp2p-related warning messages with consistent formatting
#[macro_export]
macro_rules! libp2p_warn {
    ($msg:expr) => {
        log::warn!("libp2p: {}", $msg)
    };
    ($msg:expr, $($arg:tt)*) => {
        log::warn!("libp2p: {}", format_args!($msg, $($arg)*))
    };
}

/// Macro for libp2p-related info messages with consistent formatting
#[macro_export]
macro_rules! libp2p_info {
    ($msg:expr) => {
        log::info!("libp2p: {}", $msg)
    };
    ($msg:expr, $($arg:tt)*) => {
        log::info!("libp2p: {}", format_args!($msg, $($arg)*))
    };
}

/// Macro for libp2p-related debug messages with consistent formatting
#[macro_export]
macro_rules! libp2p_debug {
    ($msg:expr) => {
        log::debug!("libp2p: {}", $msg)
    };
    ($msg:expr, $($arg:tt)*) => {
        log::debug!("libp2p: {}", format_args!($msg, $($arg)*))
    };
}

/// Macro for libp2p-related error messages with consistent formatting
#[macro_export]
macro_rules! libp2p_error {
    ($msg:expr) => {
        log::error!("libp2p: {}", $msg)
    };
    ($msg:expr, $($arg:tt)*) => {
        log::error!("libp2p: {}", format_args!($msg, $($arg)*))
    };
}

/// Structured logger for protocol-specific messages
pub struct ProtocolLogger;

impl ProtocolLogger {
    /// Log a protocol-specific warning message
    pub fn warn(protocol: &str, message: &str) {
        log::warn!("libp2p: [{}] {}", protocol, message);
    }

    /// Log a protocol-specific info message
    pub fn info(protocol: &str, message: &str) {
        log::info!("libp2p: [{}] {}", protocol, message);
    }

    /// Log a protocol-specific debug message
    pub fn debug(protocol: &str, message: &str) {
        log::debug!("libp2p: [{}] {}", protocol, message);
    }

    /// Log a protocol-specific error message
    pub fn error(protocol: &str, message: &str) {
        log::error!("libp2p: [{}] {}", protocol, message);
    }

    /// Log a failure event with structured format
    pub fn log_failure<E: std::fmt::Debug>(
        protocol: &str,
        direction: &str,
        peer: libp2p::PeerId,
        error: E,
    ) {
        log::warn!(
            "libp2p: [{}] {} failure with peer {}: {:?}",
            protocol,
            direction,
            peer,
            error
        );
    }

    /// Log a successful event with structured format
    pub fn log_success(protocol: &str, operation: &str, peer: Option<libp2p::PeerId>) {
        match peer {
            Some(peer_id) => {
                log::info!(
                    "libp2p: [{}] {} successful with peer {}",
                    protocol,
                    operation,
                    peer_id
                );
            }
            None => {
                log::info!("libp2p: [{}] {} successful", protocol, operation);
            }
        }
    }
}

/// Crypto-specific logging utilities
pub struct CryptoLogger;

impl CryptoLogger {
    /// Log encryption/decryption operations
    pub fn log_crypto_operation(operation: &str, success: bool, details: Option<&str>) {
        let status = if success { "successful" } else { "failed" };
        match details {
            Some(detail) => log::info!("crypto: {} {}: {}", operation, status, detail),
            None => log::info!("crypto: {} {}", operation, status),
        }
    }



    /// Log signature verification results
    pub fn log_signature_verification(valid: bool, peer: Option<libp2p::PeerId>) {
        match (valid, peer) {
            (true, Some(peer_id)) => {
                log::debug!("signature verification successful for peer: {}", peer_id)
            }
            (true, None) => log::debug!("signature verification successful"),
            (false, Some(peer_id)) => {
                log::warn!("signature verification failed for peer: {}", peer_id)
            }
            (false, None) => log::warn!("signature verification failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_logger_formatting() {
        // These tests verify the log format but don't check output since we're using the log crate
        ProtocolLogger::info("test_protocol", "test message");
        ProtocolLogger::warn("test_protocol", "test warning");
        ProtocolLogger::debug("test_protocol", "test debug");
        ProtocolLogger::error("test_protocol", "test error");
    }

    #[test]
    fn test_crypto_logger_operations() {
        CryptoLogger::log_crypto_operation("encryption", true, Some("AES-256"));
        CryptoLogger::log_crypto_operation("decryption", false, None);
    }
}
