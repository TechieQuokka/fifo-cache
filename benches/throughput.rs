//! Multi-threaded throughput benchmarks for cache algorithms.
//!
//! Run with: `cargo bench --bench throughput`

use std::sync::Arc;
use std::thread;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use fifo_cache::{Cache, LruCache, S3FifoCache, SieveCache};

// ============================================================================
// Benchmark Configuration
// ============================================================================

mod config {
    /// Cache capacity for benchmarks.
    pub const CAPACITY: usize = 10_000;

    /// Operations per iteration.
    pub const OPS_PER_ITER: u64 = 100_000;

    /// Key space size (larger than capacity to ensure evictions).
    pub const KEY_SPACE: u64 = 100_000;

    /// Thread counts to benchmark.
    pub const THREAD_COUNTS: &[usize] = &[1, 2, 4, 8];

    /// Read ratio for mixed workloads.
    pub const READ_RATIO: f64 = 0.8;
}

// ============================================================================
// Simple RNG for Reproducibility
// ============================================================================

struct FastRng {
    state: u64,
}

impl FastRng {
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
    fn next_key(&mut self) -> u64 {
        self.next_u64() % config::KEY_SPACE
    }

    #[inline]
    fn should_read(&mut self) -> bool {
        (self.next_u64() % 100) < (config::READ_RATIO * 100.0) as u64
    }
}

// ============================================================================
// Single-Threaded Benchmarks
// ============================================================================

fn bench_single_thread_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread_get");
    group.throughput(Throughput::Elements(config::OPS_PER_ITER));

    // S3-FIFO
    {
        let cache = S3FifoCache::new(config::CAPACITY);
        // Pre-populate
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        group.bench_function("s3fifo", |b| {
            let mut rng = FastRng::new(42);
            b.iter(|| {
                for _ in 0..config::OPS_PER_ITER {
                    let key = rng.next_u64() % config::CAPACITY as u64;
                    black_box(cache.get(&key));
                }
            });
        });
    }

    // SIEVE
    {
        let cache = SieveCache::new(config::CAPACITY);
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        group.bench_function("sieve", |b| {
            let mut rng = FastRng::new(42);
            b.iter(|| {
                for _ in 0..config::OPS_PER_ITER {
                    let key = rng.next_u64() % config::CAPACITY as u64;
                    black_box(cache.get(&key));
                }
            });
        });
    }

    // LRU
    {
        let cache = LruCache::new(config::CAPACITY);
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        group.bench_function("lru", |b| {
            let mut rng = FastRng::new(42);
            b.iter(|| {
                for _ in 0..config::OPS_PER_ITER {
                    let key = rng.next_u64() % config::CAPACITY as u64;
                    black_box(cache.get(&key));
                }
            });
        });
    }

    group.finish();
}

fn bench_single_thread_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread_insert");
    group.throughput(Throughput::Elements(config::OPS_PER_ITER));

    // S3-FIFO
    group.bench_function("s3fifo", |b| {
        let cache = S3FifoCache::new(config::CAPACITY);
        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                cache.insert(key, key);
            }
        });
    });

    // SIEVE
    group.bench_function("sieve", |b| {
        let cache = SieveCache::new(config::CAPACITY);
        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                cache.insert(key, key);
            }
        });
    });

    // LRU
    group.bench_function("lru", |b| {
        let cache = LruCache::new(config::CAPACITY);
        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                cache.insert(key, key);
            }
        });
    });

    group.finish();
}

fn bench_single_thread_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread_mixed");
    group.throughput(Throughput::Elements(config::OPS_PER_ITER));

    // S3-FIFO
    group.bench_function("s3fifo", |b| {
        let cache = S3FifoCache::new(config::CAPACITY);
        // Pre-populate
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                if rng.should_read() {
                    black_box(cache.get(&key));
                } else {
                    cache.insert(key, key);
                }
            }
        });
    });

    // SIEVE
    group.bench_function("sieve", |b| {
        let cache = SieveCache::new(config::CAPACITY);
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                if rng.should_read() {
                    black_box(cache.get(&key));
                } else {
                    cache.insert(key, key);
                }
            }
        });
    });

    // LRU
    group.bench_function("lru", |b| {
        let cache = LruCache::new(config::CAPACITY);
        for i in 0..config::CAPACITY as u64 {
            cache.insert(i, i);
        }

        let mut rng = FastRng::new(42);
        b.iter(|| {
            for _ in 0..config::OPS_PER_ITER {
                let key = rng.next_key();
                if rng.should_read() {
                    black_box(cache.get(&key));
                } else {
                    cache.insert(key, key);
                }
            }
        });
    });

    group.finish();
}

// ============================================================================
// Multi-Threaded Benchmarks
// ============================================================================

fn bench_multi_thread_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_thread_mixed");

    for &threads in config::THREAD_COUNTS {
        let ops_per_thread = config::OPS_PER_ITER / threads as u64;
        group.throughput(Throughput::Elements(config::OPS_PER_ITER));

        // S3-FIFO
        group.bench_with_input(BenchmarkId::new("s3fifo", threads), &threads, |b, &threads| {
            let cache = Arc::new(S3FifoCache::new(config::CAPACITY));
            // Pre-populate
            for i in 0..config::CAPACITY as u64 {
                cache.insert(i, i);
            }

            b.iter(|| {
                let handles: Vec<_> = (0..threads)
                    .map(|tid| {
                        let cache = Arc::clone(&cache);
                        thread::spawn(move || {
                            let mut rng = FastRng::new(tid as u64);
                            for _ in 0..ops_per_thread {
                                let key = rng.next_key();
                                if rng.should_read() {
                                    black_box(cache.get(&key));
                                } else {
                                    cache.insert(key, key);
                                }
                            }
                        })
                    })
                    .collect();

                for h in handles {
                    h.join().unwrap();
                }
            });
        });

        // SIEVE
        group.bench_with_input(BenchmarkId::new("sieve", threads), &threads, |b, &threads| {
            let cache = Arc::new(SieveCache::new(config::CAPACITY));
            for i in 0..config::CAPACITY as u64 {
                cache.insert(i, i);
            }

            b.iter(|| {
                let handles: Vec<_> = (0..threads)
                    .map(|tid| {
                        let cache = Arc::clone(&cache);
                        thread::spawn(move || {
                            let mut rng = FastRng::new(tid as u64);
                            for _ in 0..ops_per_thread {
                                let key = rng.next_key();
                                if rng.should_read() {
                                    black_box(cache.get(&key));
                                } else {
                                    cache.insert(key, key);
                                }
                            }
                        })
                    })
                    .collect();

                for h in handles {
                    h.join().unwrap();
                }
            });
        });

        // LRU
        group.bench_with_input(BenchmarkId::new("lru", threads), &threads, |b, &threads| {
            let cache = Arc::new(LruCache::new(config::CAPACITY));
            for i in 0..config::CAPACITY as u64 {
                cache.insert(i, i);
            }

            b.iter(|| {
                let handles: Vec<_> = (0..threads)
                    .map(|tid| {
                        let cache = Arc::clone(&cache);
                        thread::spawn(move || {
                            let mut rng = FastRng::new(tid as u64);
                            for _ in 0..ops_per_thread {
                                let key = rng.next_key();
                                if rng.should_read() {
                                    black_box(cache.get(&key));
                                } else {
                                    cache.insert(key, key);
                                }
                            }
                        })
                    })
                    .collect();

                for h in handles {
                    h.join().unwrap();
                }
            });
        });
    }

    group.finish();
}

// ============================================================================
// Criterion Setup
// ============================================================================

criterion_group!(
    benches,
    bench_single_thread_get,
    bench_single_thread_insert,
    bench_single_thread_mixed,
    bench_multi_thread_mixed,
);

criterion_main!(benches);
