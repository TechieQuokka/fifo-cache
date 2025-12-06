# FIFO 캐시 라이브러리 아키텍처

> S3-FIFO 및 SIEVE 캐시 제거 알고리즘의 Rust 구현
> SOSP'23 및 NSDI'24 논문 기반

## 참고 문헌

- [S3-FIFO: 캐시 제거에는 FIFO 큐만 있으면 된다 (SOSP 2023)](https://jasony.me/publication/sosp23-s3fifo.pdf)
- [SIEVE: LRU보다 간단하다 (NSDI 2024 최우수 논문)](https://www.usenix.org/conference/nsdi24/presentation/zhang-yazhuo)
- [S3-FIFO 공식 사이트](https://s3fifo.com/)
- [SIEVE 공식 사이트](https://cachemon.github.io/SIEVE-website/)
- [지연 승격과 빠른 강등의 힘](https://s3fifo.com/blog/2023/06/01/fifo-is-better-than-lru-the-power-of-lazy-promotion-and-quick-demotion/)
- [Marc Brooker의 SIEVE 분석](https://brooker.co.za/blog/2023/12/15/sieve.html)

---

## 0. 이론적 기초

### 0.1 일회성 접근 문제 (One-Hit-Wonder Problem)

프로덕션 캐시 트레이스는 중요한 통찰을 보여줍니다: **캐시된 대부분의 객체는 제거되기 전에 단 한 번만 접근됩니다**. S3-FIFO 논문은 14개 데이터셋의 6,594개 트레이스(610억 객체에 대한 8,560억 요청)를 분석했으며 다음을 발견했습니다:

| 데이터셋 | 일회성 접근 비율 | 비고 |
|---------|-----------------|------|
| Twitter | 26% | 캐시 크기 10% 기준 |
| MSR Block | 75-82% | 블록 스토리지 워크로드 |
| CDN | 40-60% | 콘텐츠 전송 |

> "캐시에 있는 대부분의 객체는 일회성 접근 객체이며, 그것들을 오랫동안 보관하는 것은 의미가 없습니다." — Yang et al., SOSP 2023

**LRU가 실패하는 이유**: LRU는 *수동적 강등*을 사용합니다 — 객체는 후속 삽입을 통해서만 큐 아래로 이동합니다. 이는 드물게 접근되는 항목이 필요 이상으로 오래 지속되어, 재사용되지 않을 객체에 캐시 공간을 낭비하게 됩니다.

### 0.2 두 가지 핵심 원칙

논문들은 효율적인 캐시 제거를 위한 두 가지 기본 원칙을 식별합니다:

#### 지연 승격 (Lazy Promotion, LP)

**정의**: 모든 접근 시 메타데이터를 업데이트하는 대신, 제거 시점까지 승격 결정을 미룹니다.

```
전통적 LRU:  접근 → 즉시 헤드로 이동 (즉시 처리)
지연 승격:   접근 → 플래그만 설정, 제거 시점에 승격 (지연 처리)
```

**장점**:
1. **계산 감소**: 히트 시 큐 재정렬 없음
2. **락 경합 감소**: 리스트 조작 대신 원자적 플래그 연산
3. **더 나은 정보**: 결정 전에 접근 패턴을 관찰할 시간이 더 많음

**실험 결과**: FIFO-재삽입(지연 승격)은 작은 캐시 크기에서 10개 데이터셋 중 9개에서 LRU를 능가합니다.

#### 빠른 강등 (Quick Demotion, QD)

**정의**: 객체가 천천히 노화되도록 두는 대신, 재사용 가능성이 없는 객체를 빠르게 제거합니다.

```
전통적:      삽입 → 전체 큐 순회 대기 → 제거
빠른 강등:   삽입 → 짧은 수습 기간 → 미사용 시 제거
```

**핵심 통찰**: 짧은 요청 시퀀스는 *더 높은* 일회성 접근 비율을 가집니다. 작은 수습 큐(용량의 10%)로 대부분의 일회성 접근 객체가 메인 캐시에 도달하기 전에 식별하고 제거할 수 있습니다.

**실험 결과**:
- QD-ARC는 ARC의 미스율을 최대 59.8% 감소
- QD-LIRS는 LIRS의 미스율을 최대 49.6% 감소

### 0.3 FIFO가 LRU를 능가하는 이유

FIFO 기반 알고리즘의 이론적 우월성은 세 가지 요인에서 비롯됩니다:

| 요인 | LRU | FIFO (LP+QD 적용) |
|------|-----|-------------------|
| **히트 연산** | O(1)이지만 락 필요 | O(1) 원자적 연산 |
| **확장성** | 4+ 스레드에서 저하 | 16+ 스레드까지 선형 |
| **미스율** | 기준선 | 최대 72% 낮음 |
| **구현** | 복잡 (이중 연결 리스트) | 단순 (큐 또는 배열) |

**처리량 분석** (논문 기준):
```
스레드:     1      4      8      16
LRU:       8M    6M     4M     2M   ops/sec (경합)
S3-FIFO:   10M   25M    40M    50M  ops/sec (선형 확장)
```

### 0.4 고스트 큐 이론

고스트 큐는 최근 제거된 객체의 *메타데이터만* (객체 ID, 값 제외) 저장합니다. 이는 다음을 제공합니다:

1. **두 번째 기회 입장**: 다시 요청된 객체는 메인 큐에 직접 삽입
2. **스캔 저항**: 순차 스캔이 캐시를 오염시키는 것을 방지
3. **메모리 효율성**: 실제 데이터가 아닌 해시만 저장

**수학적 직관**: 객체가 작은 큐에서 제거되었지만 다시 요청되면, 일회성 접근 패턴을 넘어서는 시간적 지역성을 보여준 것입니다. 이러한 객체는 메인 큐에 직접 입장할 자격이 있습니다.

### 0.5 SIEVE vs CLOCK: 결정적 차이

SIEVE와 CLOCK은 비슷해 보이지만 중요한 차이가 있습니다:

```
CLOCK:  제거 → 유지된 객체를 제거 지점에 재삽입
SIEVE:  제거 → 유지된 객체는 원래 위치에 유지
```

**위치가 중요한 이유**:

```
초기:       [A:1] [B:0] [C:1] [D:0] [E:1]  (손이 D에 위치)
                             ↑
CLOCK이 D를 제거한 후:
            [A:1] [B:0] [C:1] [D:0→새것] [E:1]
            (D 교체됨, 유지된 항목이 새것과 혼합)

SIEVE가 D를 제거한 후:
            [A:1] [B:0] [C:1] [E:1] ← [새 항목]
            (오래된 객체와 새 객체가 분리 유지)
```

**결과**: SIEVE는 FIFO의 미스율을 평균 21% 감소시키는 반면, CLOCK은 15%만 감소시킵니다.

### 0.6 워크로드 특성

| 워크로드 유형 | 최적 알고리즘 | 이유 |
|--------------|--------------|------|
| 웹/CDN 캐시 | SIEVE | 높은 시간적 지역성, 스캔 적음 |
| 블록 스토리지 | S3-FIFO | 랜덤/순차 접근 혼합 |
| 키-값 저장소 | 둘 다 | Zipf 인기 분포 |
| 데이터베이스 버퍼 | S3-FIFO | 스캔 저항 중요 |

**SIEVE 제한**: 스캔 저항이 없습니다. 대규모 순차 스캔이 작업 집합을 제거할 수 있습니다. 블록 캐시 워크로드에는 S3-FIFO를 사용하세요.

### 0.7 형식적 복잡도 분석

| 연산 | LRU | S3-FIFO | SIEVE |
|------|-----|---------|-------|
| 조회 | O(1) | O(1) | O(1) |
| 히트 업데이트 | O(1)* | O(1) 원자적 | O(1) 원자적 |
| 삽입 | O(1) | O(1) 분할상환 | O(1) |
| 제거 | O(1) | O(1) 분할상환 | O(k)** |
| 공간 | O(n) | O(n + g)*** | O(n) |

\* 리스트 조작을 위해 배타적 락 필요
\*\* k = 제거 후보를 찾기 전에 스캔한 방문 객체 수
\*\*\* g = 고스트 큐 크기 (메타데이터만)

---

## 1. 알고리즘 요약

### 1.1 S3-FIFO (Simple, Scalable, FIFO)

![S3-FIFO 아키텍처](./diagrams/s3fifo-structure.svg)

**핵심 메커니즘:**
- **작은 큐 (S)**: 용량의 10%, 일회성 접근 필터링
- **메인 큐 (M)**: 용량의 90%, 재사용 가능 객체 저장
- **고스트 큐 (G)**: 메타데이터만 (객체 ID), M과 동일한 개수
- **2비트 빈도 카운터**: 접근 횟수 추적 (최대 3)

**연산:**
```
INSERT(key, value):
    if key in ghost_queue:
        main_queue 헤드에 삽입
        ghost_queue에서 제거
    else:
        small_queue 헤드에 삽입

    while 용량 초과:
        evict()

ACCESS(key):
    if key in small_queue or main_queue:
        freq 증가 (최대 3)
        return value
    return None

EVICT():
    if small_queue가 비어있지 않음:
        obj = small_queue.pop_tail()
        if obj.freq > 1:
            obj.freq = 0
            main_queue.push_head(obj)
        else:
            ghost_queue.push_head(obj.key)
    else:
        evict_from_main()

EVICT_FROM_MAIN():
    while true:
        obj = main_queue.pop_tail()
        if obj.freq > 0:
            obj.freq -= 1
            main_queue.push_head(obj)
        else:
            return  # 객체 제거됨
```

### 1.2 SIEVE (LRU보다 간단하다)

![SIEVE 알고리즘](./diagrams/sieve-structure.svg)

**핵심 메커니즘:**

| 구성요소 | 설명 | 목적 |
|----------|------|------|
| **FIFO 큐** | 이중 연결 리스트 | 삽입 순서 유지 |
| **방문 비트** | 객체당 1비트 | 삽입 이후 접근 추적 |
| **손 포인터** | 꼬리 → 머리 이동 | 제거 후보 찾기 |

**핵심 통찰**: LRU와 달리, 캐시 히트는 **구조적 수정이 필요 없습니다** — `visited = true` 설정만 (원자적 연산).

**SIEVE vs CLOCK:**

![SIEVE vs CLOCK 비교](./diagrams/sieve-vs-clock.svg)

**연산:**
```
INSERT(key, value):
    node = create_node(key, value, visited=false)
    HEAD에 삽입                    ──────▶  O(1)

    while 용량 초과:
        evict()

ACCESS(key):                          ──────▶  O(1) 원자적!
    if key in cache:
        node.visited = true           # ← 이게 전부!
        return value
    return None

EVICT():                              ──────▶  O(1) 분할상환
    while hand.visited == true:
        hand.visited = false          # 방문 비트 초기화
        hand = hand.prev              # 헤드 방향으로 이동
        if hand is None:
            hand = tail               # 순환

    evict_node = hand
    hand = hand.prev
    remove(evict_node)                # 미방문 객체 제거
```

---

## 2. Rust 아키텍처

### 2.1 프로젝트 구조

```
fifo-cache/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 공개 API & 재내보내기
│   ├── cache/
│   │   ├── mod.rs          # Cache 트레이트 정의
│   │   ├── s3fifo.rs       # S3-FIFO 구현
│   │   ├── sieve.rs        # SIEVE 구현
│   │   └── lru.rs          # LRU 기준선 (벤치마크용)
│   ├── queue/
│   │   ├── mod.rs          # 큐 추상화
│   │   ├── fifo.rs         # 기본 FIFO 큐
│   │   └── ghost.rs        # 고스트 큐 (메타데이터만)
│   ├── node/
│   │   ├── mod.rs          # 노드 타입
│   │   └── entry.rs        # 메타데이터가 있는 캐시 엔트리
│   ├── bin/
│   │   └── fifo_bench.rs   # CLI 벤치마크 도구
│   ├── config.rs           # 설정 & 빌더 (defaults & messages 포함)
│   ├── stats.rs            # 히트/미스 통계
│   └── error.rs            # 에러 타입
├── benches/                # (생성 예정)
│   ├── throughput.rs       # 멀티스레드 처리량
│   └── hit_ratio.rs        # 캐시 효율성 벤치마크
├── examples/               # (생성 예정)
│   └── basic_usage.rs
└── documents/
    ├── ARCHITECTURE.md     # 영문 버전
    └── ARCHITECTURE_KO.md  # 이 파일
```

### 2.2 핵심 트레이트 정의

```rust
/// 핵심 캐시 트레이트 - 알고리즘 무관
pub trait Cache<K, V>: Send + Sync
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    /// 값을 가져오고, 접근 메타데이터 업데이트
    fn get(&self, key: &K) -> Option<V>;

    /// 값 삽입 또는 업데이트
    fn insert(&self, key: K, value: V) -> Option<V>;

    /// 값 제거
    fn remove(&self, key: &K) -> Option<V>;

    /// 키 존재 여부 확인
    fn contains(&self, key: &K) -> bool;

    /// 현재 엔트리 수
    fn len(&self) -> usize;

    /// 최대 용량
    fn capacity(&self) -> usize;

    /// 모든 엔트리 삭제
    fn clear(&self);

    /// 캐시 통계 가져오기
    fn stats(&self) -> CacheStats;
}
```

### 2.3 설정 패턴

```rust
/// 캐시 설정을 위한 빌더 패턴
/// rust-cli-reviewer 원칙 준수: 하드코딩된 값 없음
pub struct CacheConfig {
    /// 총 캐시 용량
    capacity: usize,

    /// S3-FIFO: 작은 큐 비율 (기본값: 0.10)
    small_queue_ratio: f64,

    /// 통계 수집 활성화 (기본값: true)
    enable_stats: bool,

    /// 초기 해시맵 용량 힌트
    initial_capacity: Option<usize>,
}

impl CacheConfig {
    pub fn new(capacity: usize) -> Self { ... }

    pub fn small_queue_ratio(mut self, ratio: f64) -> Self { ... }

    pub fn with_stats(mut self, enable: bool) -> Self { ... }

    pub fn build_s3fifo<K, V>(self) -> S3FifoCache<K, V> { ... }

    pub fn build_sieve<K, V>(self) -> SieveCache<K, V> { ... }
}
```

---

## 3. 데이터 구조

### 3.1 캐시 엔트리

```rust
/// 메타데이터가 있는 캐시에 저장된 엔트리
pub struct CacheEntry<K, V> {
    key: K,
    value: V,

    // S3-FIFO: 2비트 빈도 카운터 (0-3)
    // SIEVE: 방문 비트 (bool이지만 원자적 연산을 위해 u8 사용)
    metadata: AtomicU8,
}

impl<K, V> CacheEntry<K, V> {
    // S3-FIFO 연산
    pub fn increment_freq(&self) {
        let _ = self.metadata.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |v| if v < 3 { Some(v + 1) } else { None }
        );
    }

    pub fn decrement_freq(&self) -> u8 {
        self.metadata.fetch_sub(1, Ordering::Relaxed)
    }

    pub fn freq(&self) -> u8 {
        self.metadata.load(Ordering::Relaxed)
    }

    // SIEVE 연산
    pub fn mark_visited(&self) {
        self.metadata.store(1, Ordering::Relaxed);
    }

    pub fn clear_visited(&self) {
        self.metadata.store(0, Ordering::Relaxed);
    }

    pub fn is_visited(&self) -> bool {
        self.metadata.load(Ordering::Relaxed) != 0
    }
}
```

### 3.2 S3-FIFO 구조

```rust
use std::collections::HashMap;
use std::sync::{RwLock, Mutex};
use std::collections::VecDeque;

pub struct S3FifoCache<K, V> {
    /// 빠른 키 조회
    index: RwLock<HashMap<K, EntryLocation>>,

    /// 작은 큐: 일회성 접근 필터링
    small: Mutex<VecDeque<Arc<CacheEntry<K, V>>>>,

    /// 메인 큐: 재사용 가능성 있는 객체 저장
    main: Mutex<VecDeque<Arc<CacheEntry<K, V>>>>,

    /// 고스트 큐: 제거된 키 저장 (값 없음)
    ghost: Mutex<VecDeque<K>>,

    /// 설정
    config: CacheConfig,

    /// 통계
    stats: CacheStats,
}

#[derive(Clone, Copy)]
enum EntryLocation {
    Small(usize),
    Main(usize),
}
```

### 3.3 SIEVE 구조

```rust
use std::collections::HashMap;
use std::sync::{RwLock, Mutex};
use std::ptr::NonNull;

/// SIEVE용 이중 연결 리스트 노드
struct SieveNode<K, V> {
    entry: CacheEntry<K, V>,
    prev: Option<NonNull<SieveNode<K, V>>>,
    next: Option<NonNull<SieveNode<K, V>>>,
}

pub struct SieveCache<K, V> {
    /// 빠른 키 조회
    index: RwLock<HashMap<K, NonNull<SieveNode<K, V>>>>,

    /// 연결 리스트 헤드 (최신)
    head: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// 연결 리스트 테일 (가장 오래됨)
    tail: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// 제거용 손 포인터
    hand: Mutex<Option<NonNull<SieveNode<K, V>>>>,

    /// 현재 크기
    size: AtomicUsize,

    /// 설정
    config: CacheConfig,

    /// 통계
    stats: CacheStats,
}
```

---

## 4. 동시성 모델

### 4.1 락 전략

| 연산 | S3-FIFO | SIEVE |
|------|---------|-------|
| `get()` (히트) | RwLock 읽기 + 원자적 freq++ | RwLock 읽기 + 원자적 visited=1 |
| `get()` (미스) | RwLock 읽기만 | RwLock 읽기만 |
| `insert()` | RwLock 쓰기 + 큐 락 | RwLock 쓰기 + 리스트 락 |
| `evict()` | 큐 락 | 리스트 락 + 손 업데이트 |

### 4.2 락-프리 캐시 히트 (핵심 통찰)

두 알고리즘 모두 인기 있는 객체에 대해 **락-프리 캐시 히트**를 허용합니다:

```rust
// S3-FIFO: 인기 객체 히트 (freq가 이미 최대)
fn get(&self, key: &K) -> Option<V> {
    let index = self.index.read().unwrap();
    if let Some(location) = index.get(key) {
        let entry = self.get_entry(location);
        entry.increment_freq();  // 원자적, 큐 수정 없음!
        return Some(entry.value.clone());
    }
    None
}

// SIEVE: 방문된 객체 히트
fn get(&self, key: &K) -> Option<V> {
    let index = self.index.read().unwrap();
    if let Some(node_ptr) = index.get(key) {
        let node = unsafe { node_ptr.as_ref() };
        node.entry.mark_visited();  // 원자적, 리스트 수정 없음!
        return Some(node.entry.value.clone());
    }
    None
}
```

### 4.3 이것이 중요한 이유

- **LRU**: 모든 히트가 노드를 헤드로 이동해야 함 (락 경합)
- **S3-FIFO/SIEVE**: 히트는 원자적 메타데이터만 수정 (경합 없음)
- 결과: 16 스레드에서 **6배 처리량 향상**

---

## 5. 에러 처리

```rust
/// 캐시 전용 에러
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("용량은 0보다 커야 합니다")]
    InvalidCapacity,

    #[error("작은 큐 비율은 0.0과 1.0 사이여야 합니다")]
    InvalidSmallQueueRatio,

    #[error("다른 스레드의 패닉으로 인해 캐시가 오염되었습니다")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, CacheError>;
```

---

## 6. 통계

```rust
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
    insertions: AtomicU64,
    evictions: AtomicU64,
}

impl CacheStats {
    pub fn hit_ratio(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 { 0.0 } else { hits as f64 / total as f64 }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
}
```

---

## 7. CLI 인터페이스 (벤치마크 도구)

`rust-cli-reviewer` 가이드라인 준수:

```rust
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "fifo-cache")]
#[command(about = "FIFO 기반 캐시 알고리즘 벤치마크 도구")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 처리량 벤치마크 실행
    Bench {
        /// 벤치마크할 캐시 알고리즘
        #[arg(short, long, value_enum)]
        algorithm: Algorithm,

        /// 캐시 용량
        #[arg(short, long, default_value = "10000")]
        capacity: usize,

        /// 스레드 수
        #[arg(short, long, default_value = "4")]
        threads: usize,

        /// 연산 횟수
        #[arg(short, long, default_value = "1000000")]
        operations: usize,

        /// 출력 형식
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },

    /// 트레이스 파일 분석
    Trace {
        /// 트레이스 파일 경로
        #[arg(short, long)]
        file: PathBuf,

        /// 사용할 알고리즘
        #[arg(short, long, value_enum)]
        algorithm: Algorithm,
    },
}

#[derive(Clone, ValueEnum)]
enum Algorithm {
    S3Fifo,
    Sieve,
    Lru,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Csv,
}
```

---

## 8. 의존성

```toml
[package]
name = "fifo-cache"
version = "0.1.0"
edition = "2021"

[dependencies]
# 에러 처리
thiserror = "1.0"

# 해싱
ahash = "0.8"           # HashMap용 빠른 해시

# CLI (선택적, 벤치마크 바이너리용)
clap = { version = "4.5", features = ["derive"], optional = true }

# 직렬화 (선택적)
serde = { version = "1.0", features = ["derive"], optional = true }

[dev-dependencies]
criterion = "0.5"
rand = "0.8"
rand_distr = "0.4"     # 현실적인 워크로드를 위한 Zipf 분포

[features]
default = []
cli = ["clap"]
serde = ["dep:serde"]

[[bin]]
name = "fifo-bench"
required-features = ["cli"]
```

---

## 9. 구현 단계

### 1단계: 핵심 구현
- [ ] 원자적 메타데이터가 있는 `CacheEntry`
- [ ] `Cache` 트레이트 정의
- [ ] `SieveCache` 구현 (더 단순, 핵심 ~20줄)
- [ ] 기본 단위 테스트

### 2단계: S3-FIFO
- [ ] 3-큐 구조
- [ ] 고스트 큐 로직
- [ ] 제거 정책
- [ ] 통합 테스트

### 3단계: 동시성 & 성능
- [ ] 락 최적화
- [ ] Criterion으로 벤치마크 스위트
- [ ] Zipf 분포 워크로드
- [ ] 멀티스레드 스트레스 테스트

### 4단계: CLI & 마무리
- [ ] 벤치마크 CLI 도구
- [ ] 트레이스 파일 분석
- [ ] 문서화
- [ ] 예제

---

## 10. 설계 결정

| 결정 | 선택 | 근거 |
|------|------|------|
| 해시맵 | `ahash::HashMap` | std HashMap보다 빠름 |
| 연결 리스트 | 커스텀 unsafe 구현 | std LinkedList는 커서를 지원하지 않음 |
| 빈도 카운터 | `AtomicU8` | 락-프리 증가 |
| 설정 | 빌더 패턴 | 유연함, 하드코딩된 값 없음 |
| 에러 처리 | `thiserror` | 관용적, 제로 코스트 |
| 통계 | 기능으로 선택적 | 비활성화 시 오버헤드 없음 |

---

## 11. 성능 목표

논문 결과 기준:

| 지표 | 목표 | 기준선 (LRU) |
|------|------|--------------|
| 단일 스레드 처리량 | 10M ops/sec | 8M ops/sec |
| 16-스레드 처리량 | 50M ops/sec | 8M ops/sec (경합) |
| 미스율 (Zipf 0.9) | ≤ LRU | 기준선 |
| 메모리 오버헤드 | 데이터 대비 < 5% | ~8% (LRU 포인터) |

---

## 부록 A: 빠른 참조

### S3-FIFO vs SIEVE 비교

| 측면 | S3-FIFO | SIEVE |
|------|---------|-------|
| 복잡도 | 중간 | 낮음 (~20 LOC) |
| 데이터 구조 | 3 큐 + 고스트 | 1 리스트 + 손 |
| 엔트리당 메타데이터 | 2 비트 (freq) | 1 비트 (visited) |
| 적합한 용도 | 다양한 워크로드 | 웹 캐시 워크로드 |
| 스캔 저항 | 강함 (고스트 큐) | 보통 |
| 구현 노력 | 높음 | 낮음 |

---

## 부록 B: 최신 기술과의 알고리즘 비교

### B.1 논문에서 평가된 알고리즘

| 알고리즘 | 연도 | 핵심 아이디어 | 약점 |
|----------|------|--------------|------|
| **LRU** | 1965 | 최근성 기반 제거 | 락 경합, 낮은 확장성 |
| **CLOCK** | 1968 | 원형 버퍼 + 사용 비트 | 오래된/새 객체 혼합 |
| **2Q** | 1994 | 두 큐 (A1in, Am) | 큰 수습 큐 |
| **ARC** | 2003 | 적응형 최근성/빈도 균형 | 복잡, 특허 문제 |
| **LIRS** | 2002 | 참조 간 최근성 | 높은 메타데이터 오버헤드 |
| **TinyLFU** | 2017 | 블룸 필터 빈도 추정 | 입장 오버헤드 |
| **LeCaR** | 2018 | ML 기반 LRU/LFU 선택 | 계산 오버헤드 |
| **S3-FIFO** | 2023 | 3 FIFO 큐 + 빠른 강등 | 약간 더 복잡 |
| **SIEVE** | 2024 | FIFO + 제자리 유지 | 스캔 저항 없음 |

### B.2 미스율 비교 (논문 결과)

14개 데이터셋의 6,594개 트레이스:

```
알고리즘     N개 데이터셋에서 최고    LRU 대비 평균 미스율
─────────────────────────────────────────────────────────
S3-FIFO           10                    -6.3%
SIEVE              8                    -5.1%
TinyLFU            2                    -3.2%
ARC                1                    -2.8%
LIRS               1                    -2.1%
LeCaR              0                    -1.9%
2Q                 0                    -0.8%
LRU             기준선                    0.0%
FIFO               0                    +4.2%
```

### B.3 처리량 비교 (16 스레드)

```
알고리즘     Ops/sec (백만)    LRU 대비
────────────────────────────────────────────────────
S3-FIFO              48                  6.0×
SIEVE                45                  5.6×
CLOCK                42                  5.2×
TinyLFU              12                  1.5×
ARC                   8                  1.0×
LRU                   8                  1.0× (기준선)
LIRS                  6                  0.75×
```

### B.4 프로덕션 도입

| 시스템 | 알고리즘 | 상태 | 비고 |
|--------|----------|------|------|
| Google (내부) | S3-FIFO | 프로덕션 | 웹 캐시 |
| VMware | SIEVE | 프로덕션 | vSAN 캐싱 |
| Redpanda | S3-FIFO | 프로덕션 | 스트리밍 플랫폼 |
| TiDB | SIEVE | 통합됨 | 분산 데이터베이스 |
| Immudb | SIEVE | 통합됨 | 불변 데이터베이스 |
| 20+ OSS 라이브러리 | 둘 다 | 사용 가능 | GitHub 구현 |

---

## 부록 C: 수학적 증명과 직관

### C.1 왜 10% 작은 큐인가?

논문은 다양한 워크로드에서 10%가 최적임을 경험적으로 결정했습니다:

```
작은 큐 %    평균 미스율 (작은 큐 없는 경우 대비)
────────────────────────────────────────────────────
     5%                 -3.1%
    10%                 -4.8%  ← 최적
    15%                 -4.2%
    20%                 -3.5%
    30%                 -2.1%
```

**직관**: 너무 작으면 = 필터링 부족; 너무 크면 = 좋은 객체의 승격 지연.

### C.2 고스트 큐 크기 분석

고스트 큐는 객체 ID만 저장 (일반적으로 8바이트 vs 데이터 100+바이트):

```
고스트 크기     메모리 오버헤드    미스율 개선
───────────────────────────────────────────────────
0× main              0%                  0%
0.5× main           ~2%                +1.2%
1× main             ~4%                +2.1%  ← 기본값
2× main             ~8%                +2.3%
```

**트레이드오프**: 1× 메인 큐 크기를 넘으면 수확 체감.

### C.3 빈도 카운터 비트

S3-FIFO는 2비트 카운터 사용 (최대값 3):

```
비트    최대값    미스율    엔트리당 메모리
────────────────────────────────────────────────────
1           1          +0.3%           1 비트
2           3           0.0%           2 비트  ← 논문 선택
3           7          -0.1%           3 비트
4          15          -0.1%           4 비트
```

**통찰**: 2비트로 충분; 더 많은 비트는 무시할 만한 개선만 제공.

### C.4 SIEVE 손 이동 분석

제거당 스캔된 객체의 기대 수:

```
p = 객체가 방문될 확률이라 하면
기대 스캔 = 1/(1-p)

일반적인 웹 워크로드 (p ≈ 0.3):
기대 스캔 ≈ 1.43 객체/제거

고지역성 워크로드 (p ≈ 0.7):
기대 스캔 ≈ 3.33 객체/제거
```

**최악의 경우**: 모든 객체 방문됨 → 전체 큐 스캔 → O(n) 제거

---

## 부록 D: Zipf 분포와 캐시 워크로드

### D.1 캐싱에서의 Zipf 법칙

실제 접근 패턴은 Zipf 분포를 따릅니다:

```
P(rank = k) ∝ 1/k^α

여기서 α (비대칭도)는 일반적으로 0.7에서 1.2 범위
```

| α 값 | 특성 | 예시 워크로드 |
|------|------|--------------|
| 0.7 | 낮은 비대칭 | 블록 스토리지 |
| 0.9 | 보통 | 일반 웹 |
| 1.0 | 고전적 Zipf | CDN 트래픽 |
| 1.2 | 높은 비대칭 | 소셜 미디어 |

### D.2 알고리즘 설계에서 Zipf가 중요한 이유

```
α = 0.9, 캐시 크기 = 고유 객체의 10%

상위 10% 객체가 ~65%의 요청 수신
상위 1% 객체가 ~25%의 요청 수신
하위 50% 객체가 ~8%의 요청 수신
```

**함의**: 대부분의 객체는 드물게 접근됨 → 빠른 강등이 중요.

### D.3 벤치마크 설정

현실적인 벤치마크를 위해 사용:

```rust
use rand_distr::{Zipf, Distribution};

let zipf = Zipf::new(num_objects, 0.99).unwrap();
let key = zipf.sample(&mut rng) as u64;
```

---

## 부록 E: 관련 연구 타임라인

```
1965 ──── LRU (Denning)
   │
1968 ──── CLOCK (Corbató)
   │
1994 ──── 2Q (Johnson & Shasha)
   │
2002 ──── LIRS (Jiang & Zhang)
   │
2003 ──── ARC (Megiddo & Modha) ─── IBM 특허
   │
2017 ──── TinyLFU (Einziger et al.)
   │
2018 ──── LeCaR (Vietri et al.) ─── ML 기반
   │
2023 ──── S3-FIFO (Yang et al.) ─── SOSP 최우수 논문 후보
   │                                6,594개 트레이스 평가
   │
2024 ──── SIEVE (Zhang et al.) ─── NSDI 최우수 논문
                                    커뮤니티 상
```

**핵심 관찰**: 60년간의 연구 끝에, 지연 승격과 빠른 강등을 결합하면 단순한 FIFO 기반 알고리즘이 복잡한 대안들을 능가합니다.
