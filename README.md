# fast-prime

A highly optimized prime counting toolkit in Rust, featuring three independent implementations targeting modern hybrid-core CPUs.

## Implementations

### V1 — Segmented Sieve (`src/main.rs`)

Counts all primes up to N using a segmented Sieve of Eratosthenes with wheel mod 30 factorization, two-level cache-aware segmentation, and parallel execution via [Rayon](https://github.com/rayon-rs/rayon). Uses all 24 threads.

### V2 — Lucy_Hedgehog Combinatorial Counter (`src/bin/prime_count_v2.rs`)

Computes π(N) exactly using the Lucy_Hedgehog combinatorial method. O(N^{3/4} / ln N) time, O(√N) space — no full sieve needed. Single-threaded, yet dramatically faster than the parallel sieve for large N.

### V3 — Meissel-Lehmer (`src/bin/prime_count_v3.rs`)

Extension of V2: sieve primes only up to N^{1/3} (not N^{1/2}), then compute the remaining P₂ contribution analytically. O(N^{2/3}) time, O(√N) space. Currently the fastest implementation.

## Benchmarks — Intel Core Ultra 9 285K

```
┌─────────────┬──────────────────────────┬──────────────────────────┬──────────────────────────┬──────────────────┐
│ Range       │ V1 Sieve (24 threads)    │ V2 Lucy_Hedgehog (1 thr) │ V3 Meissel-Lehmer (1 thr)│ Primes Found     │
├─────────────┼──────────────────────────┼──────────────────────────┼──────────────────────────┼──────────────────┤
│ 1 Billion   │    0.00600s              │    0.00200s              │    0.00200s              │       50,847,534 │
│ 10 Billion  │    0.06570s              │    0.00900s              │    0.00700s              │      455,052,511 │
│ 100 Billion │    0.72087s              │    0.03500s              │    0.03400s              │    4,118,054,813 │
│ 1 Trillion  │    8.64000s              │    0.17600s              │    0.17000s              │   37,607,912,018 │
│ 10 Trillion │  127.13000s              │    1.23000s              │    1.19000s              │  346,065,536,839 │
└─────────────┴──────────────────────────┴──────────────────────────┴──────────────────────────┴──────────────────┘
```

### Best (V3) vs V1 Speedup

| Range | V1 (24 threads) | V3 (1 thread) | V3 Speedup |
|---|---|---|---|
| 1 Billion | 0.006s | 0.002s | **3.0×** |
| 10 Billion | 0.066s | 0.007s | **9.4×** |
| 100 Billion | 0.721s | 0.034s | **21.2×** |
| 1 Trillion | 8.640s | 0.170s | **50.8×** |
| 10 Trillion | 127.13s | 1.190s | **106.8×** |

### Comparison vs Strix Halo Reference

| Range | V3 (Ultra 9 285K) | Strix Halo Reference | Speedup |
|---|---|---|---|
| 1 Billion | 0.002s | 0.011s | **5.5×** |
| 10 Billion | 0.007s | 0.109s | **15.6×** |
| 100 Billion | 0.034s | 1.483s | **43.6×** |
| 1 Trillion | 0.170s | 25.820s | **151.9×** |

## Key Optimizations

### V1 — Sieve

See [OPTIMIZATIONS.md](OPTIMIZATIONS.md) for a detailed log of every optimization tried, including results (positive and negative).

- **Wheel mod 30** — Only sieves 8 residues per 30 numbers (coprime to 2, 3, 5), reducing candidate count by ~73% vs odds-only.
- **Two-level cache-aware segmentation** — L2 segments (1MB) for parallelism, with L1 sub-segments (24KB) for tiny primes (~2× speedup).
- **Extended pre-sieve pattern** — Composites of primes 7, 11, 13, 17, 19 pre-computed in a 323KB repeating pattern, tiled via memcpy.
- **4-tier prime classification** — Tiny (L1 carry-forward), small (4× unrolled), medium (simple loop), large (single-write).
- **Barrett fast division** — Precomputed reciprocals replace costly u64 division in `compute_starts`.
- **Rayon work-stealing** — Naturally balances load across P-cores and E-cores.

### V2 — Combinatorial

See [OPTIMIZATIONS_V2.md](OPTIMIZATIONS_V2.md) for the full optimization log.

- **Lucy_Hedgehog algorithm** — Computes π(N) in O(N^{3/4} / ln N) time using O(√N) space. No full sieve needed.
- **Two-phase harmonic iteration** — Splits the inner loop at √(N/p): Phase A iterates j with singleton blocks (1 div/j), Phase B iterates q downward with multi-element blocks (1 div/q, carry first_j). Halves total divisions (**41% speedup**).
- **Reciprocal table** — Precomputes `ceil(2^64/j)` for all j ≤ √N. Replaces integer DIVQ (~25 cycles) with u128 multiply+shift (~4 cycles) in Phase A and Phase B inner loops (**14% speedup**).
- **Barrett fast division** — Replaces integer DIV (~21 cycles) with multiply+shift (~4 cycles) in the small[] update loop.
- **i32 small array** — Values ≤ √N fit in i32, halving memory footprint for better cache utilization.
- **Unsafe indexing** — Bounds checks eliminated in hot loops where indices are mathematically guaranteed in-bounds.

### V3 — Meissel-Lehmer

See [OPTIMIZATIONS_V3.md](OPTIMIZATIONS_V3.md) for the full optimization log.

- **Meissel truncation** — Sieve primes only up to N^{1/3} instead of N^{1/2}, reducing the sieve loop from ~78K primes to ~1.2K primes at 1T.
- **P₂ analytic sum** — For primes p > N^{1/3}, the update to π(N) is just `Σ [large[p] - π(p-1)]` — one array lookup per prime. Proven correct because S_a(n/p) values are frozen after the partial sieve.
- **All V2 optimizations** — Reciprocal table, two-phase harmonic, 4× unroll, Barrett fast division.

## Building

Requires [Rust](https://rustup.rs/) (1.70+).

```sh
# Build everything
cargo build --release

# Run V1 (segmented sieve, uses all threads)
./target/release/prime-count

# Run V2 (Lucy_Hedgehog combinatorial, single-threaded)
./target/release/prime_count_v2

# Run V3 (Meissel-Lehmer, single-threaded)
./target/release/prime_count_v3
```

The build is configured with aggressive optimizations in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
```

And native CPU targeting in `.cargo/config.toml`:

```toml
[build]
rustflags = ["-C", "target-cpu=native"]
```

## Algorithms

### V1 — Segmented Sieve of Eratosthenes

1. **Bootstrap** — Build a small sieve up to √N using [primal](https://crates.io/crates/primal) to collect sieving primes
2. **Classify primes** — Split sieving primes into 4 tiers (tiny/small/medium/large) with precomputed Barrett reciprocals
3. **Segment** — Divide (√N, N] into 1MB L2-cache-friendly segments
4. **Parallel sieve** — Rayon dispatches segments across all cores. For each segment:
   - Pre-sieve pattern tiled from 323KB template (primes 7, 11, 13, 17, 19)
   - Tiny primes processed in 24KB L1 sub-segments for cache locality
   - Small/medium primes sieved across full L2 segment
   - Large primes: single byte write per residue
5. **Count** — Survivors (zero bits) counted with hardware `POPCNT` via `count_ones()`, 64 bits at a time
6. **Sum** — Per-segment counts reduced in parallel

### V2 — Lucy_Hedgehog Combinatorial Method

1. **Initialize** — Create two arrays of size √N: `small[j] = j-1` (integers in [2,j]) and `large[j] = ⌊N/j⌋-1` (integers in [2, ⌊N/j⌋])
2. **Sieve** — For each prime p (from primal), update all tracked values: `S(v) -= S(v/p) - π(p-1)`
   - **Branch 1**: For j where j·p ≤ √N, read directly from `large[j·p]`
   - **Branch 2**: Two-phase harmonic iteration:
     - Phase A (j ≤ √(N/p)): each j maps to a unique ⌊N/(jp)⌋, one division per j
     - Phase B (j > √(N/p)): iterate q downward, carry first_j forward, one division per q
   - **Small update**: Update `small[j]` in reverse order using Barrett fast division
3. **Result** — `large[1]` = π(N)

### V3 — Meissel-Lehmer Method

1. **Phase 1: Partial sieve** — Run Lucy_Hedgehog for primes p ≤ N^{1/3} only (~1.2K primes at 1T vs ~78K). This gives the S_a function values in small[] and large[].
2. **Phase 2: P₂ sum** — For each prime p in (N^{1/3}, √N], compute `large[p] - π(p-1)`. The S_a values are proven frozen (no prime between p_a and p modifies them). Sum these contributions.
3. **Result** — `large[1] - P₂` = π(N)

## Dependencies

- [primal](https://crates.io/crates/primal) — Bootstrap sieve for small primes ≤ √N
- [rayon](https://crates.io/crates/rayon) — Data-parallel work-stealing thread pool

## License

MIT
