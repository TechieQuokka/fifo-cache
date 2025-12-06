//! CLI benchmark tool for FIFO-based cache algorithms.
//!
//! Run with: `cargo run --bin fifo-bench --features cli -- <command>`
//!
//! ## Examples
//!
//! ```bash
//! # Run throughput benchmark with S3-FIFO
//! fifo-bench bench -a s3fifo -c 10000 -t 4 -o 1000000
//!
//! # Compare all algorithms
//! fifo-bench compare -c 10000 -o 500000
//!
//! # Run with JSON output
//! fifo-bench bench -a sieve --output json
//! ```

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};

use fifo_cache::{Cache, LruCache, S3FifoCache, SieveCache};

// ============================================================================
// Configuration Constants
// ============================================================================

mod defaults {
    /// Default cache capacity for benchmarks.
    pub const CAPACITY: usize = 10_000;

    /// Default number of threads.
    pub const THREADS: usize = 4;

    /// Default number of operations per thread.
    pub const OPERATIONS: usize = 1_000_000;

    /// Default read ratio (percentage of gets vs inserts).
    pub const READ_RATIO: f64 = 0.8;

    /// Default Zipf skewness parameter.
    pub const ZIPF_SKEW: f64 = 0.99;

    /// Key space multiplier relative to capacity.
    pub const KEY_SPACE_MULTIPLIER: usize = 10;
}

// ============================================================================
// CLI Definition
// ============================================================================

/// Benchmark tool for FIFO-based cache algorithms.
///
/// Compare S3-FIFO, SIEVE, and LRU cache performance across
/// various workloads and configurations.
#[derive(Parser)]
#[command(name = "fifo-bench")]
#[command(author = "fifo-cache contributors")]
#[command(version)]
#[command(about = "Benchmark tool for FIFO-based cache algorithms")]
#[command(long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run throughput benchmark for a single algorithm.
    Bench {
        /// Cache algorithm to benchmark.
        #[arg(short, long, value_enum)]
        algorithm: Algorithm,

        /// Cache capacity (number of entries).
        #[arg(short, long, default_value_t = defaults::CAPACITY)]
        capacity: usize,

        /// Number of threads.
        #[arg(short, long, default_value_t = defaults::THREADS)]
        threads: usize,

        /// Total number of operations.
        #[arg(short, long, default_value_t = defaults::OPERATIONS)]
        operations: usize,

        /// Read ratio (0.0 to 1.0).
        #[arg(short, long, default_value_t = defaults::READ_RATIO)]
        read_ratio: f64,

        /// Zipf skewness (higher = more skewed, 0.0 = uniform).
        #[arg(short = 'z', long, default_value_t = defaults::ZIPF_SKEW)]
        zipf_skew: f64,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },

    /// Compare all algorithms with the same configuration.
    Compare {
        /// Cache capacity (number of entries).
        #[arg(short, long, default_value_t = defaults::CAPACITY)]
        capacity: usize,

        /// Number of threads.
        #[arg(short, long, default_value_t = defaults::THREADS)]
        threads: usize,

        /// Total number of operations.
        #[arg(short, long, default_value_t = defaults::OPERATIONS)]
        operations: usize,

        /// Read ratio (0.0 to 1.0).
        #[arg(short, long, default_value_t = defaults::READ_RATIO)]
        read_ratio: f64,

        /// Zipf skewness.
        #[arg(short = 'z', long, default_value_t = defaults::ZIPF_SKEW)]
        zipf_skew: f64,

        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },

    /// Show cache statistics for a sample workload.
    Stats {
        /// Cache algorithm.
        #[arg(short, long, value_enum)]
        algorithm: Algorithm,

        /// Cache capacity.
        #[arg(short, long, default_value_t = defaults::CAPACITY)]
        capacity: usize,

        /// Number of operations.
        #[arg(short, long, default_value_t = defaults::OPERATIONS)]
        operations: usize,
    },
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum Algorithm {
    /// S3-FIFO (SOSP 2023)
    S3fifo,
    /// SIEVE (NSDI 2024)
    Sieve,
    /// LRU baseline
    Lru,
}

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Algorithm::S3fifo => write!(f, "S3-FIFO"),
            Algorithm::Sieve => write!(f, "SIEVE"),
            Algorithm::Lru => write!(f, "LRU"),
        }
    }
}

#[derive(Clone, Copy, ValueEnum, Debug)]
enum OutputFormat {
    /// Human-readable text output.
    Text,
    /// JSON format for programmatic use.
    Json,
    /// CSV format for spreadsheets.
    Csv,
}

// ============================================================================
// Benchmark Results
// ============================================================================

#[derive(Debug, Clone)]
struct BenchmarkResult {
    algorithm: String,
    capacity: usize,
    threads: usize,
    operations: usize,
    duration_ms: f64,
    throughput_ops_sec: f64,
    hit_ratio: f64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl BenchmarkResult {
    fn print_text(&self) {
        println!("=== {} Benchmark Results ===", self.algorithm);
        println!();
        println!("Configuration:");
        println!("  Capacity:    {:>12}", format_number(self.capacity));
        println!("  Threads:     {:>12}", self.threads);
        println!("  Operations:  {:>12}", format_number(self.operations));
        println!();
        println!("Performance:");
        println!("  Duration:    {:>12.2} ms", self.duration_ms);
        println!(
            "  Throughput:  {:>12} ops/sec",
            format_number(self.throughput_ops_sec as usize)
        );
        println!();
        println!("Cache Statistics:");
        println!("  Hit Ratio:   {:>12.2}%", self.hit_ratio * 100.0);
        println!("  Hits:        {:>12}", format_number(self.hits as usize));
        println!("  Misses:      {:>12}", format_number(self.misses as usize));
        println!(
            "  Evictions:   {:>12}",
            format_number(self.evictions as usize)
        );
        println!();
    }

    fn print_json(&self) {
        println!(
            r#"{{
  "algorithm": "{}",
  "config": {{
    "capacity": {},
    "threads": {},
    "operations": {}
  }},
  "performance": {{
    "duration_ms": {:.2},
    "throughput_ops_sec": {:.0}
  }},
  "statistics": {{
    "hit_ratio": {:.4},
    "hits": {},
    "misses": {},
    "evictions": {}
  }}
}}"#,
            self.algorithm,
            self.capacity,
            self.threads,
            self.operations,
            self.duration_ms,
            self.throughput_ops_sec,
            self.hit_ratio,
            self.hits,
            self.misses,
            self.evictions
        );
    }

    fn print_csv_header() {
        println!("algorithm,capacity,threads,operations,duration_ms,throughput_ops_sec,hit_ratio,hits,misses,evictions");
    }

    fn print_csv(&self) {
        println!(
            "{},{},{},{},{:.2},{:.0},{:.4},{},{},{}",
            self.algorithm,
            self.capacity,
            self.threads,
            self.operations,
            self.duration_ms,
            self.throughput_ops_sec,
            self.hit_ratio,
            self.hits,
            self.misses,
            self.evictions
        );
    }
}

// ============================================================================
// Benchmark Implementation
// ============================================================================

/// Run benchmark on a cache implementation.
fn run_benchmark<C>(
    cache: Arc<C>,
    algorithm_name: &str,
    threads: usize,
    operations: usize,
    read_ratio: f64,
    key_space: usize,
) -> BenchmarkResult
where
    C: Cache<u64, u64> + 'static,
{
    let ops_per_thread = operations / threads;
    let capacity = cache.capacity();

    // Warm up the cache
    for i in 0..capacity.min(key_space) {
        cache.insert(i as u64, i as u64);
    }

    // Reset stats after warmup
    cache.stats().reset();

    let start = Instant::now();

    let handles: Vec<_> = (0..threads)
        .map(|thread_id| {
            let cache = Arc::clone(&cache);
            let seed = thread_id as u64;

            thread::spawn(move || {
                let mut rng = SimpleRng::new(seed);

                for _ in 0..ops_per_thread {
                    let key = rng.next_zipf(key_space as u64);

                    if rng.next_f64() < read_ratio {
                        // Read operation
                        let _ = cache.get(&key);
                    } else {
                        // Write operation
                        cache.insert(key, key);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let duration = start.elapsed();
    let stats = cache.stats();

    BenchmarkResult {
        algorithm: algorithm_name.to_string(),
        capacity,
        threads,
        operations,
        duration_ms: duration.as_secs_f64() * 1000.0,
        throughput_ops_sec: operations as f64 / duration.as_secs_f64(),
        hit_ratio: stats.hit_ratio(),
        hits: stats.hits(),
        misses: stats.misses(),
        evictions: stats.evictions(),
    }
}

/// Create and run benchmark for the specified algorithm.
fn benchmark_algorithm(
    algorithm: Algorithm,
    capacity: usize,
    threads: usize,
    operations: usize,
    read_ratio: f64,
    _zipf_skew: f64,
) -> BenchmarkResult {
    let key_space = capacity * defaults::KEY_SPACE_MULTIPLIER;

    match algorithm {
        Algorithm::S3fifo => {
            let cache = Arc::new(S3FifoCache::new(capacity));
            run_benchmark(cache, "S3-FIFO", threads, operations, read_ratio, key_space)
        }
        Algorithm::Sieve => {
            let cache = Arc::new(SieveCache::new(capacity));
            run_benchmark(cache, "SIEVE", threads, operations, read_ratio, key_space)
        }
        Algorithm::Lru => {
            let cache = Arc::new(LruCache::new(capacity));
            run_benchmark(cache, "LRU", threads, operations, read_ratio, key_space)
        }
    }
}

// ============================================================================
// Simple RNG (No External Dependencies for Binary)
// ============================================================================

/// Simple xorshift64 RNG for benchmark reproducibility.
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() as f64) / (u64::MAX as f64)
    }

    /// Generate Zipf-distributed values (simplified approximation).
    fn next_zipf(&mut self, n: u64) -> u64 {
        // Simplified Zipf: use exponential distribution approximation
        let u = self.next_f64();
        let rank = (n as f64 * u.powf(1.0 / 1.5)) as u64;
        rank.min(n - 1)
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// ============================================================================
// Main Entry Point
// ============================================================================

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Bench {
            algorithm,
            capacity,
            threads,
            operations,
            read_ratio,
            zipf_skew,
            output,
        } => {
            validate_params(capacity, threads, operations, read_ratio);

            let result = benchmark_algorithm(
                algorithm, capacity, threads, operations, read_ratio, zipf_skew,
            );

            match output {
                OutputFormat::Text => result.print_text(),
                OutputFormat::Json => result.print_json(),
                OutputFormat::Csv => {
                    BenchmarkResult::print_csv_header();
                    result.print_csv();
                }
            }
        }

        Commands::Compare {
            capacity,
            threads,
            operations,
            read_ratio,
            zipf_skew,
            output,
        } => {
            validate_params(capacity, threads, operations, read_ratio);

            let algorithms = [Algorithm::S3fifo, Algorithm::Sieve, Algorithm::Lru];

            let results: Vec<_> = algorithms
                .iter()
                .map(|&alg| {
                    eprintln!("Running {} benchmark...", alg);
                    benchmark_algorithm(alg, capacity, threads, operations, read_ratio, zipf_skew)
                })
                .collect();

            match output {
                OutputFormat::Text => {
                    println!();
                    println!("╔══════════════════════════════════════════════════════════════╗");
                    println!("║           FIFO Cache Algorithm Comparison                    ║");
                    println!("╠══════════════════════════════════════════════════════════════╣");
                    println!(
                        "║ Config: {} capacity, {} threads, {} ops",
                        format_number(capacity),
                        threads,
                        format_number(operations)
                    );
                    println!("╠══════════════════════════════════════════════════════════════╣");
                    println!("║ Algorithm │  Throughput (ops/s) │ Hit Ratio │   Duration     ║");
                    println!("╠═══════════╪═════════════════════╪═══════════╪════════════════╣");

                    for r in &results {
                        println!(
                            "║ {:9} │ {:>19} │ {:>8.2}% │ {:>11.2} ms ║",
                            r.algorithm,
                            format_number(r.throughput_ops_sec as usize),
                            r.hit_ratio * 100.0,
                            r.duration_ms
                        );
                    }

                    println!("╚═══════════╧═════════════════════╧═══════════╧════════════════╝");
                    println!();

                    // Find best performers
                    if let Some(best_throughput) =
                        results.iter().max_by(|a, b| {
                            a.throughput_ops_sec.partial_cmp(&b.throughput_ops_sec).unwrap()
                        })
                    {
                        println!(
                            "Best throughput: {} ({} ops/sec)",
                            best_throughput.algorithm,
                            format_number(best_throughput.throughput_ops_sec as usize)
                        );
                    }

                    if let Some(best_hit_ratio) = results
                        .iter()
                        .max_by(|a, b| a.hit_ratio.partial_cmp(&b.hit_ratio).unwrap())
                    {
                        println!(
                            "Best hit ratio: {} ({:.2}%)",
                            best_hit_ratio.algorithm,
                            best_hit_ratio.hit_ratio * 100.0
                        );
                    }
                }

                OutputFormat::Json => {
                    println!("[");
                    for (i, r) in results.iter().enumerate() {
                        r.print_json();
                        if i < results.len() - 1 {
                            println!(",");
                        }
                    }
                    println!("]");
                }

                OutputFormat::Csv => {
                    BenchmarkResult::print_csv_header();
                    for r in &results {
                        r.print_csv();
                    }
                }
            }
        }

        Commands::Stats {
            algorithm,
            capacity,
            operations,
        } => {
            println!("Running {} with {} capacity, {} operations...", algorithm, capacity, operations);
            println!();

            let key_space = capacity * defaults::KEY_SPACE_MULTIPLIER;
            let result = benchmark_algorithm(
                algorithm,
                capacity,
                1, // Single thread for accurate stats
                operations,
                defaults::READ_RATIO,
                defaults::ZIPF_SKEW,
            );

            println!("=== {} Cache Statistics ===", algorithm);
            println!();
            println!("Configuration:");
            println!("  Capacity:   {}", format_number(capacity));
            println!("  Key space:  {}", format_number(key_space));
            println!("  Operations: {}", format_number(operations));
            println!();
            println!("Results:");
            println!("  Hit ratio:  {:.2}%", result.hit_ratio * 100.0);
            println!("  Miss ratio: {:.2}%", (1.0 - result.hit_ratio) * 100.0);
            println!("  Hits:       {}", format_number(result.hits as usize));
            println!("  Misses:     {}", format_number(result.misses as usize));
            println!("  Evictions:  {}", format_number(result.evictions as usize));
        }
    }
}

fn validate_params(capacity: usize, threads: usize, operations: usize, read_ratio: f64) {
    if capacity == 0 {
        eprintln!("Error: capacity must be greater than 0");
        std::process::exit(1);
    }
    if threads == 0 {
        eprintln!("Error: threads must be greater than 0");
        std::process::exit(1);
    }
    if operations == 0 {
        eprintln!("Error: operations must be greater than 0");
        std::process::exit(1);
    }
    if !(0.0..=1.0).contains(&read_ratio) {
        eprintln!("Error: read_ratio must be between 0.0 and 1.0");
        std::process::exit(1);
    }
}
