//! Error types for the cache library.

use thiserror::Error;

/// Errors that can occur when using the cache.
#[derive(Debug, Error)]
pub enum CacheError {
    /// Cache capacity must be greater than zero.
    #[error("invalid capacity: expected > 0, got {0}")]
    InvalidCapacity(usize),

    /// Small queue ratio must be between 0.0 and 1.0 (exclusive).
    #[error("invalid small queue ratio: expected 0.0 < ratio < 1.0, got {0}")]
    InvalidSmallQueueRatio(f64),

    /// Maximum frequency must be at least 1.
    #[error("invalid max frequency: expected >= 1, got {0}")]
    InvalidMaxFrequency(u8),

    /// A lock was poisoned due to a panic in another thread.
    #[error("cache lock poisoned: {context}")]
    Poisoned {
        /// Description of which lock was poisoned.
        context: &'static str,
    },
}

/// A specialized Result type for cache operations.
pub type Result<T> = std::result::Result<T, CacheError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CacheError::InvalidCapacity(0);
        assert!(err.to_string().contains("0"));

        let err = CacheError::InvalidSmallQueueRatio(1.5);
        assert!(err.to_string().contains("1.5"));

        let err = CacheError::Poisoned {
            context: "index lock",
        };
        assert!(err.to_string().contains("index lock"));
    }
}
