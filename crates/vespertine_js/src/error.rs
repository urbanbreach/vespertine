//! Errors surfaced to JavaScript callers.

use std::time::Duration;

/// Failure of a renderer operation. The message is what the JavaScript caller sees.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("The renderer is not initialized; call init() first")]
    NotInitialized,
    #[error("The renderer is already initialized")]
    AlreadyInitialized,
    #[error("The GPUI UI thread is not running")]
    NotRunning,
    #[error("Timed out after {} seconds waiting for {operation}", .after.as_secs_f64())]
    TimedOut {
        operation: &'static str,
        after: Duration,
    },
    #[error("The GPUI UI thread stopped during {0}")]
    Stopped(&'static str),
    /// The mutation batch was rejected before anything changed (APP-006 R3).
    #[error("{0}")]
    Batch(String),
    /// The batch was applied to the retained tree but the window could not be asked to
    /// repaint (APP-005 R9).
    #[error("batch applied, but the repaint request failed: {0}")]
    RepaintFailed(Box<HostError>),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}
