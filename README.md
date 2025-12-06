# fifo-cache

High-performance cache implementations based on cutting-edge research papers.

## Algorithms

- **S3-FIFO**: Simple, Scalable FIFO-based cache from [SOSP 2023](https://jasony.me/publication/sosp23-s3fifo.pdf)
- **SIEVE**: Simpler than LRU eviction algorithm from [NSDI 2024 Best Paper](https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo)

## Key Features

- **Lock-free cache hits**: Both algorithms only update atomic metadata on hits
- **6× throughput vs LRU**: Scales linearly to 16+ threads
- **Up to 72% lower miss ratio**: Validated on 6,594 production traces

## Quick Start

```rust
use fifo_cache::{Cache, CacheConfig};

// Create a SIEVE cache (simpler, great for web workloads)
let cache = CacheConfig::new(10_000)
    .build_sieve::<String, Vec<u8>>()
    .unwrap();

cache.insert("key".to_string(), vec![1, 2, 3]);
assert_eq!(cache.get(&"key".to_string()), Some(vec![1, 2, 3]));

// Check hit ratio
println!("Hit ratio: {:.2}%", cache.stats().hit_ratio() * 100.0);
```

## Algorithm Selection

| Workload | Recommended | Reason |
|----------|-------------|--------|
| Web/CDN cache | SIEVE | High temporal locality |
| Block storage | S3-FIFO | Scan resistance |
| Key-value store | Either | Zipfian distribution |
| Database buffer | S3-FIFO | Mixed access patterns |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
fifo-cache = "0.1"
```

## Features

- `cli` - Enable command-line benchmark tool
- `serde` - Enable serialization support

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## References

- [S3-FIFO: FIFO Queues are All You Need for Cache Eviction (SOSP 2023)](https://jasony.me/publication/sosp23-s3fifo.pdf)
- [SIEVE is Simpler than LRU: an Efficient Turn-Key Eviction Algorithm for Web Caches (NSDI 2024)](https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo)
