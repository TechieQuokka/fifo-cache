//! Basic usage example for fifo-cache.
//!
//! Run with: `cargo run --example basic_usage`

use fifo_cache::{Cache, CacheConfig, S3FifoCache, SieveCache};

fn main() {
    println!("=== fifo-cache Basic Usage Example ===\n");

    // Example 1: Quick start with SIEVE
    example_sieve_quick_start();

    // Example 2: S3-FIFO with configuration
    example_s3fifo_configured();

    // Example 3: Using the Cache trait polymorphically
    example_polymorphic_usage();

    // Example 4: Cache statistics
    example_statistics();

    // Example 5: get_or_insert_with pattern
    example_get_or_insert();
}

/// Example 1: Quick start with SIEVE (simplest usage)
fn example_sieve_quick_start() {
    println!("--- Example 1: SIEVE Quick Start ---");

    // Create a SIEVE cache with 100 entries capacity
    let cache = SieveCache::new(100);

    // Insert some key-value pairs
    cache.insert("user:1".to_string(), "Alice".to_string());
    cache.insert("user:2".to_string(), "Bob".to_string());
    cache.insert("user:3".to_string(), "Charlie".to_string());

    // Retrieve values
    if let Some(name) = cache.get(&"user:1".to_string()) {
        println!("Found user:1 = {}", name);
    }

    // Check if key exists
    if cache.contains(&"user:2".to_string()) {
        println!("user:2 exists in cache");
    }

    println!("Cache size: {}/{}\n", cache.len(), cache.capacity());
}

/// Example 2: S3-FIFO with custom configuration
fn example_s3fifo_configured() {
    println!("--- Example 2: S3-FIFO with Configuration ---");

    // Use builder pattern for custom configuration
    let cache = CacheConfig::new(1000)
        .with_small_queue_ratio(0.15) // 15% small queue (default is 10%)
        .unwrap()
        .with_stats(true)
        .build_s3fifo::<i32, Vec<u8>>()
        .unwrap();

    // Insert binary data
    cache.insert(1, vec![0x48, 0x65, 0x6c, 0x6c, 0x6f]); // "Hello"
    cache.insert(2, vec![0x57, 0x6f, 0x72, 0x6c, 0x64]); // "World"

    // Retrieve and decode
    if let Some(data) = cache.get(&1) {
        let text = String::from_utf8_lossy(&data);
        println!("Key 1 contains: {}", text);
    }

    // Update existing key
    let old_value = cache.insert(1, vec![0x48, 0x69]); // Replace with "Hi"
    if let Some(old) = old_value {
        println!("Replaced old value: {:?}", old);
    }

    // Remove a key
    if let Some(removed) = cache.remove(&2) {
        println!("Removed key 2: {:?}\n", removed);
    }
}

/// Example 3: Using the Cache trait for polymorphism
fn example_polymorphic_usage() {
    println!("--- Example 3: Polymorphic Usage ---");

    // Function that works with any cache implementation
    fn populate_cache<C: Cache<String, i32>>(cache: &C, n: usize) {
        for i in 0..n {
            cache.insert(format!("key_{}", i), i as i32);
        }
    }

    fn access_cache<C: Cache<String, i32>>(cache: &C, keys: &[&str]) {
        for key in keys {
            if let Some(value) = cache.get(&key.to_string()) {
                println!("  {} = {}", key, value);
            } else {
                println!("  {} = NOT FOUND", key);
            }
        }
    }

    // Works with SIEVE
    println!("Using SIEVE:");
    let sieve = SieveCache::new(50);
    populate_cache(&sieve, 100);
    access_cache(&sieve, &["key_0", "key_50", "key_99"]);

    // Works with S3-FIFO
    println!("Using S3-FIFO:");
    let s3fifo = S3FifoCache::new(50);
    populate_cache(&s3fifo, 100);
    access_cache(&s3fifo, &["key_0", "key_50", "key_99"]);

    println!();
}

/// Example 4: Using cache statistics
fn example_statistics() {
    println!("--- Example 4: Cache Statistics ---");

    let cache = SieveCache::new(10);

    // Simulate a workload
    for i in 0..20 {
        cache.insert(i, i * 10);
    }

    // Access some keys multiple times
    for _ in 0..5 {
        cache.get(&5);
        cache.get(&7);
        cache.get(&9);
    }

    // Try to access some keys that were evicted
    cache.get(&0);
    cache.get(&1);
    cache.get(&2);

    // Get statistics
    let stats = cache.stats();
    println!("Cache Statistics:");
    println!("  Hits:       {}", stats.hits());
    println!("  Misses:     {}", stats.misses());
    println!("  Hit Ratio:  {:.2}%", stats.hit_ratio() * 100.0);
    println!("  Insertions: {}", stats.insertions());
    println!("  Evictions:  {}", stats.evictions());

    // Reset statistics if needed
    stats.reset();
    println!("  (Stats reset)\n");
}

/// Example 5: get_or_insert_with pattern
fn example_get_or_insert() {
    println!("--- Example 5: get_or_insert_with ---");

    let cache = SieveCache::new(100);

    // Simulate expensive computation
    fn expensive_computation(key: &str) -> String {
        println!("  Computing value for '{}'...", key);
        format!("computed_value_for_{}", key)
    }

    // First access - will compute
    let key = "important_data".to_string();
    let value = cache.get_or_insert_with(key.clone(), || expensive_computation(&key));
    println!("  Result: {}", value);

    // Second access - will use cached value (no computation)
    let value = cache.get_or_insert_with(key.clone(), || {
        panic!("This should not be called!")
    });
    println!("  Cached: {}", value);

    println!("\n=== Examples Complete ===");
}
