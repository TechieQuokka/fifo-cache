//! Cache configuration with builder pattern.
//!
//! All configuration values are explicitly documented with their rationale.
//! No hardcoded magic numbers - constants are named and explained.

use crate::error::{CacheError, Result};

/// Default configuration values based on paper recommendations.
pub mod defaults {
    /// Default small queue ratio (10% of total capacity).
    ///
    /// From S3-FIFO paper: Empirically optimal across 6,594 production traces.
    /// - 5%: -3.1% miss ratio improvement
    /// - 10%: -4.8% miss ratio improvement (optimal)
    /// - 15%: -4.2% miss ratio improvement
    pub const SMALL_QUEUE_RATIO: f64 = 0.10;

    /// Default ghost queue size multiplier relative to main queue.
    ///
    /// Ghost queue at 1× main queue size provides good scan resistance
    /// with ~4% memory overhead. Diminishing returns beyond 1×.
    pub const GHOST_QUEUE_MULTIPLIER: f64 = 1.0;

    /// Maximum frequency counter value (2-bit counter: 0-3).
    ///
    /// From paper analysis:
    /// - 1 bit (max 1): +0.3% miss ratio
    /// - 2 bits (max 3): baseline (chosen)
    /// - 3+ bits: negligible improvement
    pub const MAX_FREQUENCY: u8 = 3;

    /// Default cache capacity when using the Default trait.
    ///
    /// Chosen as a reasonable starting point for small applications.
    /// For production use, explicitly configure via `CacheConfig::new(capacity)`.
    pub const DEFAULT_CAPACITY: usize = 1024;

    /// Tolerance for floating-point comparisons in tests.
    pub const FLOAT_TOLERANCE: f64 = 0.01;
}

/// Common error messages for consistent error handling.
pub mod messages {
    /// Error message for zero capacity.
    pub const ZERO_CAPACITY: &str = "capacity must be greater than 0";
}

/// Cache configuration builder.
///
/// Provides a fluent API for configuring cache behavior.
/// All defaults are based on paper recommendations.
///
/// # Example
///
/// ```rust
/// use fifo_cache::CacheConfig;
///
/// let config = CacheConfig::new(10_000)
///     .with_small_queue_ratio(0.10)
///     .unwrap()
///     .with_stats(true);
///
/// let cache = config.build_sieve::<String, Vec<u8>>().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Total cache capacity (number of entries).
    capacity: usize,

    /// Ratio of small queue to total capacity (S3-FIFO only).
    small_queue_ratio: f64,

    /// Ghost queue size multiplier relative to main queue (S3-FIFO only).
    ghost_queue_multiplier: f64,

    /// Maximum frequency counter value (S3-FIFO only).
    max_frequency: u8,

    /// Whether to collect hit/miss statistics.
    enable_stats: bool,

    /// Initial hash map capacity hint.
    initial_capacity: Option<usize>,
}

impl CacheConfig {
    /// Create a new configuration with the specified capacity.
    ///
    /// Uses default values from the S3-FIFO and SIEVE papers.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of entries the cache can hold.
    ///
    /// # Panics
    ///
    /// Does not panic. Use `validate()` or `build_*()` to check for errors.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            small_queue_ratio: defaults::SMALL_QUEUE_RATIO,
            ghost_queue_multiplier: defaults::GHOST_QUEUE_MULTIPLIER,
            max_frequency: defaults::MAX_FREQUENCY,
            enable_stats: true,
            initial_capacity: None,
        }
    }

    /// Set the ratio of small queue to total capacity.
    ///
    /// Only affects S3-FIFO caches. Must be between 0.0 and 1.0 (exclusive).
    ///
    /// # Arguments
    ///
    /// * `ratio` - Fraction of capacity for small queue (default: 0.10).
    ///
    /// # Errors
    ///
    /// Returns an error if ratio is not in (0.0, 1.0).
    pub fn with_small_queue_ratio(mut self, ratio: f64) -> Result<Self> {
        if ratio <= 0.0 || ratio >= 1.0 {
            return Err(crate::error::CacheError::InvalidSmallQueueRatio(ratio));
        }
        self.small_queue_ratio = ratio;
        Ok(self)
    }

    /// Set the ghost queue size multiplier.
    ///
    /// Only affects S3-FIFO caches. Multiplied by main queue capacity.
    ///
    /// # Arguments
    ///
    /// * `multiplier` - Size multiplier (default: 1.0, same as main queue).
    #[must_use]
    pub fn with_ghost_queue_multiplier(mut self, multiplier: f64) -> Self {
        self.ghost_queue_multiplier = multiplier;
        self
    }

    /// Set the maximum frequency counter value.
    ///
    /// Only affects S3-FIFO caches. Higher values use more memory per entry.
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum frequency value (default: 3, uses 2 bits).
    ///
    /// # Errors
    ///
    /// Returns an error if max is zero.
    pub fn with_max_frequency(mut self, max: u8) -> Result<Self> {
        if max == 0 {
            return Err(crate::error::CacheError::InvalidMaxFrequency(max));
        }
        self.max_frequency = max;
        Ok(self)
    }

    /// Enable or disable statistics collection.
    ///
    /// When disabled, hit/miss counters are not updated, reducing overhead.
    ///
    /// # Arguments
    ///
    /// * `enable` - Whether to collect statistics (default: true).
    #[must_use]
    pub fn with_stats(mut self, enable: bool) -> Self {
        self.enable_stats = enable;
        self
    }

    /// Set the initial hash map capacity hint.
    ///
    /// Pre-allocates space to avoid rehashing during initial population.
    ///
    /// # Arguments
    ///
    /// * `hint` - Expected number of entries (default: same as capacity).
    #[must_use]
    pub fn with_initial_capacity(mut self, hint: usize) -> Self {
        self.initial_capacity = Some(hint);
        self
    }

    /// Validate the configuration.
    ///
    /// Called automatically by `build_*()` methods.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Capacity is zero
    /// - Small queue ratio is not in (0.0, 1.0)
    /// - Max frequency is zero
    pub fn validate(&self) -> Result<()> {
        if self.capacity == 0 {
            return Err(CacheError::InvalidCapacity(self.capacity));
        }

        if self.small_queue_ratio <= 0.0 || self.small_queue_ratio >= 1.0 {
            return Err(CacheError::InvalidSmallQueueRatio(self.small_queue_ratio));
        }

        if self.max_frequency == 0 {
            return Err(CacheError::InvalidMaxFrequency(self.max_frequency));
        }

        Ok(())
    }

    // === Getters ===

    /// Get the total cache capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get the small queue ratio.
    #[must_use]
    pub fn small_queue_ratio(&self) -> f64 {
        self.small_queue_ratio
    }

    /// Get the ghost queue multiplier.
    #[must_use]
    pub fn ghost_queue_multiplier(&self) -> f64 {
        self.ghost_queue_multiplier
    }

    /// Get the maximum frequency value.
    #[must_use]
    pub fn max_frequency(&self) -> u8 {
        self.max_frequency
    }

    /// Check if statistics are enabled.
    #[must_use]
    pub fn stats_enabled(&self) -> bool {
        self.enable_stats
    }

    /// Get the initial capacity hint.
    #[must_use]
    pub fn initial_capacity_hint(&self) -> usize {
        self.initial_capacity.unwrap_or(self.capacity)
    }

    // === Computed values ===

    /// Calculate small queue capacity.
    #[must_use]
    pub fn small_queue_capacity(&self) -> usize {
        ((self.capacity as f64) * self.small_queue_ratio).ceil() as usize
    }

    /// Calculate main queue capacity.
    #[must_use]
    pub fn main_queue_capacity(&self) -> usize {
        self.capacity.saturating_sub(self.small_queue_capacity())
    }

    /// Calculate ghost queue capacity.
    #[must_use]
    pub fn ghost_queue_capacity(&self) -> usize {
        ((self.main_queue_capacity() as f64) * self.ghost_queue_multiplier).ceil() as usize
    }

    // === Builders ===

    /// Build an S3-FIFO cache with this configuration.
    ///
    /// # Type Parameters
    ///
    /// * `K` - Key type (must be `Hash + Eq + Clone + Send + Sync`)
    /// * `V` - Value type (must be `Clone + Send + Sync`)
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails.
    pub fn build_s3fifo<K, V>(self) -> Result<crate::S3FifoCache<K, V>>
    where
        K: std::hash::Hash + Eq + Clone + Send + Sync,
        V: Clone + Send + Sync,
    {
        self.validate()?;
        Ok(crate::S3FifoCache::with_config(self))
    }

    /// Build a SIEVE cache with this configuration.
    ///
    /// # Type Parameters
    ///
    /// * `K` - Key type (must be `Hash + Eq + Clone + Send + Sync`)
    /// * `V` - Value type (must be `Clone + Send + Sync`)
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails.
    pub fn build_sieve<K, V>(self) -> Result<crate::SieveCache<K, V>>
    where
        K: std::hash::Hash + Eq + Clone + Send + Sync,
        V: Clone + Send + Sync,
    {
        self.validate()?;
        Ok(crate::SieveCache::with_config(self))
    }
}

impl Default for CacheConfig {
    /// Create a default configuration with default capacity.
    ///
    /// Uses `defaults::DEFAULT_CAPACITY` (1024 entries).
    fn default() -> Self {
        Self::new(defaults::DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let config = CacheConfig::new(1000);
        assert_eq!(config.capacity(), 1000);
        assert!((config.small_queue_ratio() - defaults::SMALL_QUEUE_RATIO).abs() < f64::EPSILON);
        assert!(
            (config.ghost_queue_multiplier() - defaults::GHOST_QUEUE_MULTIPLIER).abs()
                < f64::EPSILON
        );
        assert_eq!(config.max_frequency(), defaults::MAX_FREQUENCY);
        assert!(config.stats_enabled());
    }

    #[test]
    fn test_builder_chain() {
        let config = CacheConfig::new(1000)
            .with_small_queue_ratio(0.15)
            .unwrap()
            .with_ghost_queue_multiplier(0.5)
            .with_max_frequency(7)
            .unwrap()
            .with_stats(false)
            .with_initial_capacity(500);

        assert!((config.small_queue_ratio() - 0.15).abs() < f64::EPSILON);
        assert!((config.ghost_queue_multiplier() - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.max_frequency(), 7);
        assert!(!config.stats_enabled());
        assert_eq!(config.initial_capacity_hint(), 500);
    }

    #[test]
    fn test_validation_errors() {
        assert!(CacheConfig::new(0).validate().is_err());
        assert!(CacheConfig::new(100).with_small_queue_ratio(0.0).is_err());
        assert!(CacheConfig::new(100).with_small_queue_ratio(1.0).is_err());
        assert!(CacheConfig::new(100).with_small_queue_ratio(-0.1).is_err());
        assert!(CacheConfig::new(100).with_max_frequency(0).is_err());
    }

    #[test]
    fn test_queue_capacities() {
        let config = CacheConfig::new(100).with_small_queue_ratio(0.10).unwrap();
        assert_eq!(config.small_queue_capacity(), 10);
        assert_eq!(config.main_queue_capacity(), 90);
        assert_eq!(config.ghost_queue_capacity(), 90);
    }

    #[test]
    fn test_queue_capacities_with_multiplier() {
        let config = CacheConfig::new(100)
            .with_small_queue_ratio(0.10)
            .unwrap()
            .with_ghost_queue_multiplier(0.5);
        assert_eq!(config.ghost_queue_capacity(), 45);
    }
}
