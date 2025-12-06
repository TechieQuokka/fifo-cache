//! Hit ratio benchmarks comparing cache algorithms under different workloads.
//!
//! Run with: `cargo bench --bench hit_ratio`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use fifo_cache::{Cache, LruCache, S3FifoCache, SieveCache};

// ============================================================================
// Benchmark Configuration
// ============================================================================

mod config {
    /// Cache sizes to test (percentage of key space).
    pub const CACHE_SIZE_RATIOS: &[f64] = &[0.01, 0.05, 0.10, 0.20];

    /// Total key space size.
    pub const KEY_SPACE: u64 = 100_000;

    /// Operations to run for each test.
    pub const OPERATIONS: u64 = 500_000;
}

// ============================================================================
// Workload Generators
// ============================================================================

/// Simple xorshift64 RNG.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    #[inline]
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() as f64) / (u64::MAX as f64)
    }
}

/// Generates keys following Zipf distribution.
fn generate_zipf_workload(n: usize, key_space: u64, skew: f64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let u = rng.next_f64();
            let rank = (key_space as f64 * u.powf(1.0 / (1.0 + skew))) as u64;
            rank.min(key_space - 1)
        })
        .collect()
}

/// Generates uniform random keys.
fn generate_uniform_workload(n: usize, key_space: u64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| rng.next_u64() % key_space).collect()
}

/// Generates a workload with hot/cold regions.
fn generate_hotspot_workload(n: usize, key_space: u64, hot_ratio: f64, seed: u64) -> Vec<u64> {
    let mut rng = Rng::new(seed);
    let hot_keys = (key_space as f64 * 0.1) as u64; // 10% of keys are hot

    (0..n)
        .map(|_| {
            if rng.next_f64() < hot_ratio {
                // Access hot keys
                rng.next_u64() % hot_keys
            } else {
                // Access cold keys
                hot_keys + (rng.next_u64() % (key_space - hot_keys))
            }
        })
        .collect()
}

// ============================================================================
// Hit Ratio Measurement
// ============================================================================

struct HitRatioResult {
    algorithm: String,
    capacity: usize,
    hit_ratio: f64,
    operations: u64,
}

fn measure_hit_ratio<C: Cache<u64, u64>>(
    cache: &C,
    workload: &[u64],
    algorithm: &str,
) -> HitRatioResult {
    // Clear stats
    cache.stats().reset();
    cache.clear();

    // Run workload
    for &key in workload {
        if cache.get(&key).is_none() {
            cache.insert(key, key);
        }
    }

    let stats = cache.stats();

    HitRatioResult {
        algorithm: algorithm.to_string(),
        capacity: cache.capacity(),
        hit_ratio: stats.hit_ratio(),
        operations: stats.total_accesses(),
    }
}

// ============================================================================
// Benchmarks
// ============================================================================

fn bench_zipf_hit_ratio(c: &mut Criterion) {
    let mut group = c.benchmark_group("hit_ratio_zipf");
    group.sample_size(10); // Fewer samples since these are deterministic

    let workload = generate_zipf_workload(
        config::OPERATIONS as usize,
        config::KEY_SPACE,
        0.99,
        12345,
    );

    for &ratio in config::CACHE_SIZE_RATIOS {
        let capacity = (config::KEY_SPACE as f64 * ratio) as usize;

        // S3-FIFO
        group.bench_with_input(
            BenchmarkId::new("s3fifo", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = S3FifoCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "S3-FIFO"));
            },
        );

        // SIEVE
        group.bench_with_input(
            BenchmarkId::new("sieve", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = SieveCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "SIEVE"));
            },
        );

        // LRU
        group.bench_with_input(
            BenchmarkId::new("lru", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = LruCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "LRU"));
            },
        );
    }

    group.finish();
}

fn bench_uniform_hit_ratio(c: &mut Criterion) {
    let mut group = c.benchmark_group("hit_ratio_uniform");
    group.sample_size(10);

    let workload =
        generate_uniform_workload(config::OPERATIONS as usize, config::KEY_SPACE, 12345);

    for &ratio in config::CACHE_SIZE_RATIOS {
        let capacity = (config::KEY_SPACE as f64 * ratio) as usize;

        group.bench_with_input(
            BenchmarkId::new("s3fifo", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = S3FifoCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "S3-FIFO"));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sieve", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = SieveCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "SIEVE"));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("lru", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = LruCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "LRU"));
            },
        );
    }

    group.finish();
}

fn bench_hotspot_hit_ratio(c: &mut Criterion) {
    let mut group = c.benchmark_group("hit_ratio_hotspot");
    group.sample_size(10);

    // 80% of accesses go to 10% of keys
    let workload =
        generate_hotspot_workload(config::OPERATIONS as usize, config::KEY_SPACE, 0.8, 12345);

    for &ratio in config::CACHE_SIZE_RATIOS {
        let capacity = (config::KEY_SPACE as f64 * ratio) as usize;

        group.bench_with_input(
            BenchmarkId::new("s3fifo", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = S3FifoCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "S3-FIFO"));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sieve", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = SieveCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "SIEVE"));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("lru", format!("{:.0}%", ratio * 100.0)),
            &capacity,
            |b, &capacity| {
                let cache = LruCache::new(capacity);
                b.iter(|| measure_hit_ratio(&cache, &workload, "LRU"));
            },
        );
    }

    group.finish();
}

// ============================================================================
// Quick Hit Ratio Report (not a benchmark, just measurement)
// ============================================================================

fn print_hit_ratio_report() {
    println!("\n=== Hit Ratio Report ===\n");

    let capacity = 10_000;
    let ops = 500_000;

    let zipf_workload = generate_zipf_workload(ops, config::KEY_SPACE, 0.99, 12345);
    let uniform_workload = generate_uniform_workload(ops, config::KEY_SPACE, 12345);
    let hotspot_workload = generate_hotspot_workload(ops, config::KEY_SPACE, 0.8, 12345);

    println!("Capacity: {} (10% of key space {})", capacity, config::KEY_SPACE);
    println!("Operations: {}\n", ops);

    println!("┌────────────┬────────────┬────────────┬────────────┐");
    println!("│ Workload   │   S3-FIFO  │    SIEVE   │     LRU    │");
    println!("├────────────┼────────────┼────────────┼────────────┤");

    // Zipf
    let s3 = measure_hit_ratio(&S3FifoCache::new(capacity), &zipf_workload, "S3-FIFO");
    let sieve = measure_hit_ratio(&SieveCache::new(capacity), &zipf_workload, "SIEVE");
    let lru = measure_hit_ratio(&LruCache::new(capacity), &zipf_workload, "LRU");
    println!(
        "│ Zipf 0.99  │   {:>6.2}%  │   {:>6.2}%  │   {:>6.2}%  │",
        s3.hit_ratio * 100.0,
        sieve.hit_ratio * 100.0,
        lru.hit_ratio * 100.0
    );

    // Uniform
    let s3 = measure_hit_ratio(&S3FifoCache::new(capacity), &uniform_workload, "S3-FIFO");
    let sieve = measure_hit_ratio(&SieveCache::new(capacity), &uniform_workload, "SIEVE");
    let lru = measure_hit_ratio(&LruCache::new(capacity), &uniform_workload, "LRU");
    println!(
        "│ Uniform    │   {:>6.2}%  │   {:>6.2}%  │   {:>6.2}%  │",
        s3.hit_ratio * 100.0,
        sieve.hit_ratio * 100.0,
        lru.hit_ratio * 100.0
    );

    // Hotspot
    let s3 = measure_hit_ratio(&S3FifoCache::new(capacity), &hotspot_workload, "S3-FIFO");
    let sieve = measure_hit_ratio(&SieveCache::new(capacity), &hotspot_workload, "SIEVE");
    let lru = measure_hit_ratio(&LruCache::new(capacity), &hotspot_workload, "LRU");
    println!(
        "│ Hotspot    │   {:>6.2}%  │   {:>6.2}%  │   {:>6.2}%  │",
        s3.hit_ratio * 100.0,
        sieve.hit_ratio * 100.0,
        lru.hit_ratio * 100.0
    );

    println!("└────────────┴────────────┴────────────┴────────────┘\n");
}

fn bench_report(c: &mut Criterion) {
    // Print report once at the start
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(print_hit_ratio_report);

    // Dummy benchmark to satisfy criterion
    c.bench_function("hit_ratio_report_printed", |b| {
        b.iter(|| {
            // No-op, report already printed
        });
    });
}

// ============================================================================
// Criterion Setup
// ============================================================================

criterion_group!(
    benches,
    bench_report,
    bench_zipf_hit_ratio,
    bench_uniform_hit_ratio,
    bench_hotspot_hit_ratio,
);

criterion_main!(benches);
