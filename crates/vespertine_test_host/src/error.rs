use thiserror::Error;

/// Errors reported by the test host.
#[derive(Debug, Error)]
pub enum HostError {
    /// A window dimension was not a positive finite number that fits in 32-bit signed geometry.
    #[error(
        "invalid window {dimension}: {value} (must be a finite number greater than 0 and at \
         most 2147483647)"
    )]
    InvalidSize {
        /// `"width"` or `"height"`.
        dimension: &'static str,
        /// The rejected value.
        value: f64,
    },
    /// No GPU test backend could be created. The host never substitutes a tree-only, mock or
    /// software-reconstructed renderer.
    #[error("GPU test backend unavailable: {0}")]
    BackendUnavailable(String),
    /// A retained-tree batch was rejected.
    #[error("{0}")]
    Batch(String),
    /// A keystroke token could not be parsed.
    #[error("invalid keystroke {token:?}: {reason}")]
    InvalidKeystroke {
        /// The offending token.
        token: String,
        /// Why it was rejected.
        reason: String,
    },
    /// A mouse button index outside 0 (left), 1 (middle), 2 (right).
    #[error("invalid mouse button {0}: expected 0 (left), 1 (middle) or 2 (right)")]
    InvalidButton(u8),
    /// A duration that was negative, NaN or infinite.
    #[error("invalid duration {0}: must be a finite number of milliseconds >= 0")]
    InvalidDuration(f64),
    /// An element identity that is not in the retained tree.
    #[error("no element with id {0}")]
    NoSuchElement(u64),
    /// The operation needs a capability the hosted content does not provide (for example a
    /// retained tree, or a feature of an element type).
    #[error("{0}")]
    Unsupported(String),
    /// Bytes that are not a font.
    #[error("invalid font: {0}")]
    InvalidFont(String),
    /// The offscreen window is gone or rejected an update.
    #[error("window error: {0}")]
    Window(String),
    /// A screenshot could not be read back or saved.
    #[error("screenshot failed: {0}")]
    Screenshot(String),
}

/// Result alias for the test host.
pub type HostResult<T> = Result<T, HostError>;
