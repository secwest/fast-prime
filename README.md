# fast-prime

A highly optimized prime counting toolkit in Rust, featuring seven independent implementations targeting modern hybrid-core CPUs.

## Implementations

### V1 — Segmented Sieve (`src/main.rs`)

Counts all primes up to N using a segmented Sieve of Eratosthenes with wheel mod 30 factorization, two-level cache-aware segmentation, and parallel execution via [Rayon](https://github.com/rayon-rs/rayon). Uses all 24 threads.

### V2 — Lucy_Hedgehog Combinatorial Counter (`src/bin/prime_count_v2.rs`)

Computes π(N) exactly using the Lucy_Hedgehog combinatorial method. O(N^{3/4} / ln N) time, O(√N) space — no full sieve needed. Single-threaded, yet dramatically faster than the parallel sieve for large N.

### V3 — Meissel-Lehmer (`src/bin/prime_count_v3.rs`)

Extension of V2: sieve primes only up to N^{1/3} (not N^{1/2}), then compute the remaining P₂ contribution analytically. O(N^{2/3}) time, O(√N) space.

### V4 — Lagarias-Miller-Odlyzko (`src/bin/prime_count_v4.rs`)

Full LMO prime counting with segmented sieve for special leaves. O(N^{2/3} / log N) time, O(N^{1/3}) space. Parallel S2 via delta-phi correction, parallel P2 via custom multi-threaded sieve. Currently the fastest implementation, beating V3 by 54× at 10T.

### V5 — Deleglise-Rivat (`src/bin/prime_count_v5.rs`)

Deleglise-Rivat prime counting: splits special leaves into easy (π-table lookup) and hard (sieve-based), reducing sieve work. Same O(N^{2/3} / log N) complexity but fewer sieve iterations for large N.

### V6 — Enhanced DR with Segmented Pi Table (`src/bin/prime_count_v6.rs`)

Gourdon-inspired enhancement of V5: processes the π-table in L2-cache-sized segments instead of requiring the full table to fit in L3. Eliminates the y-cap constraint, allowing larger y values that dramatically reduce hard-leaf sieve work. Segmented P2 replaces the multi-GB monolithic sieve with a 2-4MB cache-friendly approach. ValidM list pre-filters squarefree numbers for 8.5× fewer Type 1 iterations. Falls back to V5's direct approach for small inputs.

### V7 — Gourdon's Algorithm (`src/bin/prime_count_v7.rs`)

Full implementation of Gourdon's 2001 algorithm: π(x) = AC - B + D + Φ₀ + Σ. Uses two independent alpha parameters (α_y for y, α_z for z) instead of V6's single alpha, allowing independent tuning of the easy-leaf and hard-leaf domains. D (hard leaves) processes fewer iterations than V6's S2_hard by using tighter x* bounds. Parallel D via chunk-based phi correction with L2-cache-sized segments. BigPiTable provides O(1) π(n) lookups for AC and B with software prefetch. ValidM list pre-filters D Type 1 for 8.5× fewer iterations. Piecewise-linear alpha lookup table tuned per-scale. **Currently the fastest implementation — 6.8× faster than V6 at Max i64.**

## Benchmarks — Intel Core Ultra 9 285K

```
┌───────────────┬──────────────┬──────────────┬──────────────┬──────────────┬──────────────┬──────────────┬──────────────┬───────────────────┐
│ Range         │ V1 Sieve     │ V2 Lucy_HH   │ V3 Meissel   │ V4 LMO       │ V5 DR        │ V6 Seg-Pi    │ V7 Gourdon   │ Primes Found      │
│               │ (24 threads) │ (1 thread)   │ (1 thread)   │ (parallel)   │ (parallel)   │ (parallel)   │ (parallel)   │                   │
├───────────────┼──────────────┼──────────────┼──────────────┼──────────────┼──────────────┼──────────────┼──────────────┼───────────────────┤
│ 1 Billion     │    0.00600s  │    0.00200s  │    0.00200s  │    0.00090s  │    0.00080s  │    0.00097s  │    0.00171s  │        50,847,534 │
│ 10 Billion    │    0.06570s  │    0.00900s  │    0.00700s  │    0.00200s  │    0.00300s  │    0.00175s  │    0.00170s  │       455,052,511 │
│ 100 Billion   │    0.72087s  │    0.03500s  │    0.03400s  │    0.00300s  │    0.00500s  │    0.00311s  │    0.00237s  │     4,118,054,813 │
│ 1 Trillion    │    8.64000s  │    0.17600s  │    0.16800s  │    0.00600s  │    0.01600s  │    0.01323s  │    0.00446s  │    37,607,912,018 │
│ 10 Trillion   │  127.13000s  │    1.23000s  │    1.19000s  │    0.01900s  │    0.02800s  │    0.06099s  │    0.01349s  │   346,065,536,839 │
│ 100 Trillion  │ 2389.23000s  │    8.07000s  │    7.83000s  │    0.09100s  │    0.09200s  │    0.21665s  │    0.04406s  │ 3,204,941,750,802 │
│ 1 Quadrillion │          —   │   42.51000s  │   40.57000s  │    0.75500s  │    0.53900s  │    0.51846s  │    0.17337s  │29,844,570,422,669 │
│ 10 Quadrillion│          —   │          —   │  208.33000s  │    5.43000s  │    3.41000s  │    2.31895s  │    0.65938s  │279,238,341,033,925│
│ 100 Quadrillion│         —   │          —   │          —   │   33.63000s  │   21.00000s  │   14.37864s  │    1.07000s  │2,623,557,157,654,233│
│ 1 Quintillion │          —   │          —   │          —   │  192.00000s  │  172.36000s  │   51.84000s  │    3.29000s  │24,739,954,287,740,860│
│ Max i64       │          —   │          —   │          —   │  939.21000s  │          —   │  342.46000s  │   12.33000s  │216,289,611,853,439,384│
└───────────────┴──────────────┴──────────────┴──────────────┴──────────────┴──────────────┴──────────────┴──────────────┴───────────────────┘
```

### V7 vs Kim Walisch's primecount v8.2 (Gourdon, state of the art)

| Scale | V7 (Opt 34) | primecount | Ratio | primesieve |
|---|---|---|---|---|
| 1e10 | 0.004s | — | — | 0.058s |
| 1e11 | 0.004s | — | — | 0.596s |
| 1e12 | 0.006s | 0.014s | **0.4×** ✓ | 6.85s |
| 1e13 | 0.014s | 0.015s | **0.9×** ✓ | 83s |
| 1e14 | 0.029s | 0.023s | 1.3× | — |
| 1e15 | 0.079s | 0.059s | 1.3× | — |
| 1e16 | 0.261s | 0.178s | 1.5× | — |
| 1e17 | 0.932s | 0.598s | 1.6× | — |
| 1e18 | 3.38s | 2.27s | 1.5× | — |
| Max i64 | 12.33s | 8.49s | 1.45× | — |

V7 uses primesieve (Kim Walisch) as the B sieve engine via FFI streaming merge, with alpha parameters tuned through 34 rounds of optimization. primecount is the fastest published prime counting code. V7 Opt 34 is **faster at 1e12-1e13**, with the gap at **1.45× at Max i64**.

### Best (V7) vs V1 Speedup

| Range | V1 (24 threads) | V7 Gourdon (parallel) | Speedup |
|---|---|---|---|
| 1 Billion | 0.006s | 0.002s | **3.5×** |
| 10 Billion | 0.066s | 0.002s | **38.6×** |
| 100 Billion | 0.721s | 0.003s | **245.3×** |
| 1 Trillion | 8.640s | 0.008s | **1134.0×** |
| 10 Trillion | 127.13s | 0.031s | **4101.3×** |
| 100 Trillion | 2389.23s | 0.108s | **22122.5×** |
| 1 Quadrillion | — | 0.143s | — |
| 10 Quadrillion | — | 0.600s | — |
| 100 Quadrillion | — | 2.741s | — |
| 1 Quintillion | — | 10.820s | — |

### V7 vs V6 Speedup

| Range | V6 Seg-Pi | V7 Gourdon | Speedup |
|---|---|---|---|
| 1 Trillion | 0.013s | 0.004s | **3.0×** |
| 10 Trillion | 0.061s | 0.013s | **4.5×** |
| 100 Trillion | 0.217s | 0.044s | **4.9×** |
| 1 Quadrillion | 0.518s | 0.173s | **3.0×** |
| 10 Quadrillion | 2.319s | 0.659s | **3.5×** |
| 100 Quadrillion | 14.379s | 2.762s | **5.2×** |
| 1 Quintillion | 51.840s | 11.888s | **4.4×** |
| Max i64 | 342.460s | 50.121s | **6.8×** |

### Comparison vs Strix Halo Reference

| Range | V7 (Ultra 9 285K) | Strix Halo Reference | Speedup |
|---|---|---|---|
| 1 Billion | 0.002s | 0.011s | **6.4×** |
| 10 Billion | 0.002s | 0.109s | **64.1×** |
| 100 Billion | 0.003s | 1.483s | **504.6×** |
| 1 Trillion | 0.008s | 25.820s | **3389.8×** |
| 10 Trillion | 0.014s | — | — |
| 100 Trillion | 0.039s | — | — |

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

### V4 — Lagarias-Miller-Odlyzko

See [OPTIMIZATIONS_V4.md](OPTIMIZATIONS_V4.md) for the full optimization log.

- **LMO formula** — π(x) = S1 + S2 + π(y) - 1 - P2, where y = x^{1/3} · α. Completely different algorithmic approach from V2/V3.
- **S2 segmented sieve** — Special leaves computed via bit-packed sieve with POPCNT. Processes segments of [0, z] where z = x/y, crossing off primes progressively.
- **Monotonic max_b optimization** — Since max_b decreases across segments, primes beyond max_b are never processed in later segments. This eliminates O(π(y)) redundant work per segment (**2-3× speedup**).
- **Alpha tuning** — Adaptive alpha scales with log₁₀(x): α=2.2 for x≤10¹³, ramping to α=6.0 at 10¹⁵. Larger inputs benefit from higher alpha because S2 inner loops amortize per-prime overhead better (**64% speedup at 1Q**).
- **Concurrent S2+P2** — P2 runs in a background thread overlapping with S2 via `thread::scope`. Makes P2 essentially free (**18% speedup**).
- **Parallel S2 via delta-phi correction** — Segments split across threads, each tracking local phi + correction coefficients. True phi reconstructed via prefix-sum after join. Exact correction, no approximation (**1.8× speedup**).
- **Pre-sieve template** — 30030-bit precomputed pattern for primes 2,3,5,7,11,13 applied via word-aligned AND, replacing 6 individual cross-off loops (**1.9× speedup**).
- **Parallel P2 sieve** — Custom `ParallelPiSieve` replaces single-threaded `primal::Sieve`. Bitmap built in parallel via `par_chunks_mut`, with popcount prefix sums for O(1) π(n) queries (**21% speedup at 10T**).
- **Incremental count** — Positions in special leaf loops are monotonically increasing. `count_delta(prev, pos)` scans only the gap between consecutive positions instead of from 0 each time (**20% speedup at 10T**).
- **PhiTiny cache** — Precomputed wheel for φ(x, c) with c ≤ 6, giving O(1) ordinary leaf evaluation.
- **Larger segment size** — 128K-bit minimum (16KB) fits L1 cache while reducing per-segment overhead (**5% speedup**).
- **4× unrolled cross-off** — Reduces loop control overhead in the sieve cross-off inner loop (**7% speedup**).
- **Barrett fast division** — Precomputed reciprocals for ~4600 primes (37KB, fits L1). Replaces 25-cycle hardware division with 12-cycle multiply-high in easy leaf loop (**8% speedup at 10T**).
- **Deferred total update** — Cross-off loop accumulates delta locally, breaking the serial dependency on `sieve.total`. Combined with raw pointer access for zero-overhead sieve updates.
- **mimalloc allocator** — Replaces Windows default heap with mimalloc for 8.6× faster multi-threaded allocation. Reduces parallel S2 allocation overhead from 129ms to ~15ms (**5% speedup at 10T**).
- **Pi-formula for easy leaves** — For segment 0, when primes[b-1]² ≥ segment_size, uses identity phi(n,b-1) = 1 + max(pi(n)-(b-1), 0) with a precomputed pi table. Eliminates sieve counting for 4.7M easy leaf iterations. Batch-counts trivial phi=1 leaves in O(1) per prime (**10% speedup at 10T**).

### V5 — Deleglise-Rivat

See [OPTIMIZATIONS_V5.md](OPTIMIZATIONS_V5.md) for the full optimization log.

- **Easy/hard leaf split** — Special leaves classified into easy (π-table lookup) and hard (sieve-based). For large x, most leaves are easy, bypassing the expensive segmented sieve entirely.
- **S2_easy π-table identity** — φ(n, b-1) = π(n) - b + 2 when n < p_b², computed via precomputed π table. O(1) per leaf vs O(segment) sieve work.
- **Clustered batch evaluation** — Consecutive easy leaves sharing the same π(x/pq) value are counted in a single multiply, reducing per-leaf overhead.
- **Parallel S2_easy** — Easy leaf computation parallelized across b values via rayon. Each b is independent (no sieve state to share).
- **All V4 infrastructure** — PhiTiny cache, parallel P2 sieve, pre-sieve template, BitSieve with incremental counting, Barrett fast division.

### V6 — Enhanced DR with Segmented Pi Table

See [OPTIMIZATIONS_V6.md](OPTIMIZATIONS_V6.md) for the full optimization log.

- **Segmented π-table processing** — Instead of requiring the full π-table to fit in L3 cache (36MB, capping y at 9M), processes it in L2-sized segments of 128K entries (512KB). All π lookups hit L2 cache (5ns) instead of L3/DRAM (20-100ns).
- **Uncapped y parameter** — With segmented processing, y is no longer constrained by cache size. Larger y dramatically reduces S2_hard work by shrinking z = x/y. At 10^18: y goes from 9M→23M, halving z and the number of hard-leaf sieve segments.
- **Adaptive dispatch** — Automatically uses V5's direct approach (parallel over b, with prefetch) when the π-table fits in L3, switching to segmented approach for larger scales. Best of both worlds.
- **Narrowed b-range per segment** — Each segment computes the maximum valid b value (p_b² ≤ x/seg_low), skipping irrelevant b iterations.
- **Alpha tuning for segmented regime** — Higher alpha (23 at 10^18, up from 19) balances S2_easy/S2_hard perfectly with the segmented approach.
- **2.33× faster at 1 Quintillion** — 82.6s vs V4's 192.0s. 2.09× faster than V5's 172.4s.

### V7 — Gourdon's Algorithm

See [OPTIMIZATIONS_V7.md](OPTIMIZATIONS_V7.md) for the full optimization log.

- **Two-parameter alpha**— Independent α_y (controls y = x^{1/3}·α_y) and α_z (controls z = y·α_z) allow separate tuning of easy-leaf and hard-leaf domains. Piecewise-linear interpolation table with 9 data points tuned per-scale.
- **Tighter x* bounds** — D (hard leaves) uses x* = max(x^{1/4}, ⌈x/y²⌉) instead of √y, processing fewer iterations than V6's S2_hard.
- **BigPiTable** — O(1) π(n) lookups for AC and B via parallel segmented sieve with word-granularity prefix sums (u64 for large scales). Covers [0, √x] (~285MB at Max i64).
- **Parallel B** — Builds dedicated BigPiTable covering [0, x/smallest_prime], then parallel π(x/p) lookups via rayon par_iter. Runs concurrently with D+AC.
- **ValidM list for D** — Pre-filters squarefree y-smooth m values with lpf > primes[c+1]. Binary search for valid range per (b, segment). 8.5× fewer Type 1 iterations.
- **Sigma corrections** — 7 arithmetic formulas (Σ₀-Σ₆) computed in O(x^{1/3}) time, replacing expensive sieve work.
- **Parallel D with phi correction** — Segments split into chunks across threads. Each chunk tracks local phi + coefficients. True phi reconstructed via prefix-sum correction pass (same approach as V4's parallel S2).
- **Concurrent B/AC/D** — B, AC, and D run concurrently via `thread::scope`, with D using rayon internally for further parallelism.
- **3.6-5.5× faster than V6 at Max i64** — 62s vs 342s. Consistent 3.6-5.5× speedup across all scales ≥ 100Q.

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

# Run V4 (LMO, parallel S2 — fastest)
./target/release/prime_count_v4

# Run V5 (Deleglise-Rivat, parallel easy leaves)
./target/release/prime_count_v5

# Run V6 (Enhanced DR with segmented pi table)
./target/release/prime_count_v6

# Run V7 (Gourdon's algorithm — fastest at all scales)
./target/release/prime_count_v7
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

### V4 — Lagarias-Miller-Odlyzko (LMO)

1. **Precompute tables** — Generate primes, least-prime-factor (lpf), Möbius function (μ), and π lookup tables up to y = x^{1/3} · α
2. **S1 (ordinary leaves)** — Σ μ(n)·φ(x/n, c) for squarefree n ≤ y with lpf(n) > p_c. Uses PhiTiny wheel cache for O(1) evaluation
3. **S2 (special leaves)** — Segmented sieve over [0, x/y]:
   - For each segment, pre-sieve first c primes
   - Hard leaves (b ≤ π(√y)): iterate squarefree m values, count unsieved via POPCNT
   - Easy leaves (b > π(√y)): two-prime products, iterate primes q via π table
   - Progressive cross-off maintains sieve state φ(pos, b-1)
4. **P2 (prime pairs)** — Σ π(x/p) for primes p in (y, √x], using primal sieve
5. **Result** — π(x) = S1 + S2 + π(y) - 1 - P2

### V5 — Deleglise-Rivat Method

1. **Precompute tables** — Same as V4: primes, lpf, μ, π lookup tables up to y = x^{1/3} · α
2. **S1 (ordinary leaves)** — Same as V4: Σ μ(n)·φ(x/n, c) using PhiTiny cache
3. **S2_easy (easy special leaves)** — For b > π(√y), leaves where x/(p_b·p_l) ≤ y:
   - Uses identity φ(n, b-1) = π(n) - b + 2 (valid since p_b > √y → p_b² > y ≥ n)
   - Clusters consecutive l values with same π value for batch computation
   - Handles trivial leaves (φ=1 when x/(p·q) < p_b) separately
   - Parallelized across b values via rayon
4. **S2_hard (hard special leaves)** — Segmented sieve over [0, x/y]:
   - Type 1 (b ≤ π(√y)): All leaves via sieve, same as V4
   - Type 2 (π(√y) < b ≤ π(√z)): Only hard leaves (x/(p·q) ≥ y) via sieve
5. **P2 (prime pairs)** — Same as V4
6. **Result** — π(x) = S1 + S2_easy + S2_hard + π(y) - 1 - P2

### V6 — Enhanced DR with Segmented Pi Table

Same algorithmic formula as V5, with a key implementation innovation:

1. **Precompute tables** — Same as V5, but y is NOT capped at 9M. Larger y reduces z = x/y, cutting S2_hard work
2. **S1 (ordinary leaves)** — Same as V5
3. **S2_easy (adaptive)** — Dispatches based on π-table size:
   - **Small tables (≤ L3)**: V5's direct approach — parallel over b values, software prefetch, clustering
   - **Large tables (> L3)**: Segmented approach — divides π[0..y] into 2MB segments, processes each segment in parallel. For each segment, finds all (b, l) pairs where x/(p_b·p_l) falls in that segment range, ensuring all π lookups hit L2 cache
4. **S2_hard (hard special leaves)** — Same as V5, benefits from smaller z
5. **P2 (prime pairs)** — Same as V5
6. **Result** — π(x) = S1 + S2_easy + S2_hard + π(y) - 1 - P2

### V7 — Gourdon's Algorithm

1. **Parameters** — Two independent alpha values: y = x^{1/3}·α_y, z = y·α_z, x* = max(x^{1/4}, ⌈x/y²⌉)
2. **Precompute tables** — Primes up to y, π table up to max(z, √(x/x*)), Möbius/lpf/y-smooth tables up to z, BigPiTable covering [0, √x]
3. **Σ (Sigma)** — 7 cheap arithmetic correction formulas (Σ₀-Σ₆), O(x^{1/3}) time
4. **Φ₀ (ordinary leaves)** — Recursive Möbius sum over squarefree numbers ≤ z with lpf > p_k, using PhiTiny cache
5. **B (prime pairs)** — Σ π(x/p) for primes y < p ≤ √x, using parallel segmented sieve
6. **AC (combined easy leaves)** — Three sub-formulas:
   - C1: recursive Möbius-weighted leaves (b ≤ π(√z))
   - C2: π-table-based easy leaves (π(√z) < b ≤ π(x*))
   - A: simplest easy leaves (π(x*) < b ≤ π(x^{1/3}))
   - All use BigPiTable for O(1) π lookups
7. **D (hard leaves)** — Segmented sieve over [0, x/z]:
   - Type 1 (b ≤ π(√z)): squarefree m leaves with μ(m)≠0, lpf(m)>p, all factors ≤ y
   - Type 2 (π(√z) < b ≤ π(x*)): prime pair leaves
   - Parallel via chunk-based phi correction
8. **Result** — π(x) = AC - B + D + Φ₀ + Σ

## Dependencies

- [primal](https://crates.io/crates/primal) — Bootstrap sieve for small primes ≤ √N
- [rayon](https://crates.io/crates/rayon) — Data-parallel work-stealing thread pool

## License

MIT
