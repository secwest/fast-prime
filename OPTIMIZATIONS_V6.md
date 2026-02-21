# V6 Optimization Log — Enhanced DR with Segmented Pi Table

## Architecture

V6 builds on V5 (Deleglise-Rivat) with a key innovation from the Gourdon algorithm:
**segmented π-table processing** for S2_easy.

Same formula: π(x) = S1 + S2_easy + S2_hard + π(y) - 1 - P2

### The Problem V6 Solves

In V5, S2_easy performs random lookups into π[0..y], where y can be up to 19M at 10^18.
The π-table at y=19M is 76MB — far exceeding the 36MB L3 cache. This creates two issues:

1. **Random DRAM lookups** — Each π(x/(p·q)) access may miss L3, hitting DRAM at ~100ns
2. **Y-cap workaround** — V5 caps y at 9M so π-table fits in L3 (36MB), but this doubles z = x/y, increasing S2_hard work

### V6's Solution: Segmented Pi Table

Instead of random access across 76MB, V6 divides π[0..y] into 512K-entry segments (2MB each, fits in L2 cache). For each segment [low, high):

1. All π lookups within that segment hit L2 cache (~5ns vs ~100ns)
2. For each prime b, computes the valid l range where x/(p_b·p_l) ∈ [low, high)
3. Processes only the relevant (b, l) pairs — same total work, but cache-friendly

### Adaptive Dispatch

- **Pi table ≤ L3 (36MB, y ≤ 9M)**: Uses V5's direct approach (parallel over b, software prefetch)
- **Pi table > L3**: Uses segmented approach (parallel over segments)

This ensures optimal performance at all scales.

---

## Opt 0 — Baseline: Segmented Pi Table + Uncapped Y

**What**: Created V6 with segmented S2_easy and removed the y=9M cap from V5.

**Implementation**:
- `compute_s2_easy_segmented()`: Parallel over segments via rayon. Each segment is 512K entries (2MB).
  For each segment, iterates over valid b values with narrowed b-range (max_b_seg = π(√(x/seg_low))).
  Uses clustering optimization within each segment for batch evaluation.
- `compute_s2_easy_direct()`: Copy of V5's S2_easy with prefetch, used when pi table fits in L3.
- `compute_s2_easy()`: Dispatcher — chooses direct vs segmented based on `(y+1)*4 ≤ 36MB`.
- `count_primes()`: No y-cap. Alpha curve unchanged, y determined purely by α·∛x.

**Results (V6 Opt 0 vs V5 Opt 5)**:

| Range | V5 | V6 | Speedup | Notes |
|-------|-----|-----|---------|-------|
| 1 Billion | 0.0008s | 0.002s | 0.4× | V5 faster (direct path) |
| 10 Billion | 0.003s | 0.002s | 1.7× | Slight improvement |
| 100 Billion | 0.005s | 0.003s | 1.7× | Direct path used |
| 1 Trillion | 0.009s | 0.007s | 1.4× | Direct path used |
| 10 Trillion | 0.029s | 0.025s | 1.2× | Direct path used |
| 100 Trillion | 0.092s | 0.089s | 1.03× | Direct path used |
| 1 Quadrillion | 0.539s | 0.518s | 1.04× | Direct path used |
| 10 Quadrillion | 3.41s | 3.52s | 0.97× | Near breakeven |
| 100 Quadrillion | 21.00s | 20.72s | 1.01× | Near breakeven |
| **1 Quintillion** | **172.4s** | **113.4s** | **1.52×** | Segmented pi kicks in! |
| **Max i64** | — | **547.3s** | — | First V6-only measurement |

**Analysis**:
- For scales ≤ 100Q: Pi table fits in L3 → V6 uses direct path → performance ≈ V5
- At 1 Quintillion: y=19M uncapped (vs V5's 9M cap), pi table=76MB → segmented approach
  - S2_easy: segmented pi gives L2-fast lookups instead of DRAM random access
  - S2_hard: z halved (53B vs 111B) → ~50% fewer sieve segments
  - Combined: **34% faster** (113.4s vs 172.4s)
- Max i64 (9.2×10^18): 547.3s, first measurement for V5/V6 algorithm at this scale
  - V4 was 939.2s → V6 is **1.72× faster than V4**

**Key insight**: The segmented approach eliminates the fundamental tension between
S2_easy cache efficiency and S2_hard work volume. V5 had to cap y at 9M to keep the
pi table in L3, doubling S2_hard work. V6 processes the pi table efficiently at any
size, allowing y to grow naturally, which dramatically reduces S2_hard.

---

## Opt 1 — Alpha Tuning + Segment Size Optimization

**What**: Two complementary tuning changes that together yield **27% speedup** at 1 Quintillion.

### Alpha Tuning

**Problem**: The alpha curve from V5 (max alpha=19 at 10^18) was suboptimal for V6. Since V6's
segmented S2_easy can handle large pi tables efficiently, higher alpha (= larger y, smaller z)
is beneficial — it reduces S2_hard work without penalizing S2_easy.

**Methodology**: Exhaustive sweep at 1 Quintillion testing alpha = 12, 15, 19, 20, 21, 22, 23, 24, 25, 26, 28, 30, 35, 40, 50.

**Results**:

| Alpha | y | z | S2_easy | S2_hard | Total |
|-------|---|---|---------|---------|-------|
| 19 | 19M | 52.6B | 98s | 122s | 123s |
| 21 | 21M | 47.6B | — | — | 99s |
| **23** | **23M** | **43.5B** | **98s** | **98s** | **97s** |
| 25 | 25M | 40.0B | — | — | 101s |
| 28 | 28M | 35.7B | — | — | 102s |
| 50 | 50M | 20.0B | — | — | 134s |

Alpha=23 balances S2_easy and S2_hard perfectly at 1 Quintillion.

**Updated alpha curve** for log_x > 17:
```
log_x 17-18: 16.0 + 7.0 * (log_x - 17)  → ramps from 16 to 23
log_x > 18:  23.0 + 5.0 * (log_x - 18)   → at Max i64 (log_x=18.96): alpha=27.8
```

### Segment Size Optimization

**Problem**: Default 512K-entry segments (2MB) may not be optimal for L2 cache utilization.
Smaller segments complete faster, freeing rayon threads for S2_hard earlier.

**Methodology**: Swept segment sizes at 1 Quintillion with alpha=23:

| Segment Size | Data Size | S2_easy | S2_hard | Total |
|-------------|-----------|---------|---------|-------|
| 32K entries | 128KB | 45.5s | 80.9s | **81.9s** |
| 64K entries | 256KB | 54.7s | 87.1s | 88.1s |
| **128K entries** | **512KB** | **50.8s** | **81.2s** | **82.2s** |
| 256K entries | 1MB | 73.3s | 84.5s | 85.5s |
| 512K entries | 2MB | 98.1s | 98.1s | 99.1s |
| 1M entries | 4MB | 162.2s | 162.2s | 163.2s |

**Key insight**: With 128K segments, S2_easy finishes in ~51s (vs 98s), freeing threads for
S2_hard. The total is limited by S2_hard (~81s). Further reducing segment size to 32K gives
diminishing returns (81.9s ≈ 82.2s) — S2_hard is the bottleneck regardless.

Chose 128K as default: optimal cache behavior (512KB fits comfortably in 2MB L2), enough
segments for good parallelization, and consistent performance.

**Combined Results (Opt 1 vs Opt 0)**:

| Range | V6 Opt 0 | V6 Opt 1 | Speedup |
|-------|----------|----------|---------|
| 1 Billion | 0.002s | 0.001s | same |
| 100 Trillion | 0.089s | 0.089s | same |
| 1 Quadrillion | 0.518s | 0.527s | same |
| 10 Quadrillion | 3.52s | 3.56s | same |
| 100 Quadrillion | 20.72s | 21.31s | same |
| **1 Quintillion** | **113.4s** | **82.6s** | **1.37×** |
| **Max i64** | **547.3s** | **502.4s** | **1.09×** |

No regressions at any scale. At 1 Quintillion: **27% faster** (113.4s → 82.6s).
At Max i64: **8% faster** (547.3s → 502.4s).

**Profiling at 1 Quintillion** (Opt 1):
- S1: 0.044s (negligible)
- S2_easy: 50.6s (finishes early, threads freed for S2_hard)
- S2_hard: 81.7s ← **new bottleneck**
- P2: 48.5s (finishes within S2 window)
- Wall time: 82.6s ≈ S2_hard time

**Next bottleneck**: S2_hard at 81s. Potential avenues:
- Fenwick tree optimization (cache-aware layout)
- Improved sieve crossing patterns
- Better load balancing across segments in S2_hard

---

## Opt 2 — Segmented P2 (Cache-Friendly π Computation)

**What**: Replaced the monolithic ParallelPiSieve (5.4GB at 1Q) with a cache-friendly
segmented sieve that processes 1M-number segments sequentially.

### The Problem

The old P2 computation allocated a full sieve covering all numbers up to z = x/y:
- At 1 Quintillion: z = 43.5B → 2.7GB bitmap + 2.7GB prefix sums = **5.4GB**
- At Max i64: z = 158B → 9.8GB bitmap + 9.8GB prefix sums = **19.6GB**
- Used rayon parallelism, stealing threads from concurrent S2_easy/S2_hard work
- Massive memory bandwidth pressure, poor cache utilization

### The Solution

Sort all x/p queries by value, then sweep a 1M-number segmented sieve from 0 upward,
resolving queries as each segment completes:

1. Pre-compute all (index, x/p) pairs and sort by x/p
2. For each 1M segment: sieve composites, build prefix sums, resolve pending queries
3. Maintain running π count across segments

**Memory**: ~2-4MB vs 5.4GB (1350× reduction at 1Q)
**Threading**: Sequential (no rayon), freeing all threads for S2_easy/S2_hard

### Bug Fixes

Two boundary bugs fixed during development:

1. **Segment boundary edge case**: When x/p falls exactly at a segment boundary (even,
   multiple of segment size), `largest_odd = xp_val - 1` falls below the segment start.
   The original code incorrectly used `bit_idx = 0`, counting the first odd number in
   the segment. Fix: check `largest_odd <= seg_low` and use `running_pi` directly.

2. **Last segment termination**: When max_xp is even and exactly at a segment boundary,
   `odd_count = 0` caused early loop exit without processing the final query.
   Fix: round `sieve_end` up to `(max_xp | 1) + 1` to ensure valid last segment.

### Results (Opt 2 vs Opt 1)

| Range | V6 Opt 1 | V6 Opt 2 | Speedup | P2 old→new |
|-------|----------|----------|---------|------------|
| 1 Billion | 0.001s | 0.001s | same | — |
| 100 Trillion | 0.089s | 0.089s | same | — |
| 10 Quadrillion | 3.56s | 2.71s | 1.31× | 1.8→0.9s |
| 100 Quadrillion | 21.31s | 14.30s | 1.49× | 7.6→4.2s |
| **1 Quintillion** | **82.6s** | **61.6s** | **1.34×** | 48.6→31.0s |
| **Max i64** | **502.4s** | **400.8s** | **1.25×** | 158→135s |

**Profiling at 1 Quintillion** (Opt 2):
- S1: 0.05s
- S2_easy: 24.6s (freed from P2 thread pressure)
- S2_hard: 60.7s ← **bottleneck** (improved from 81.7s due to no P2 thread contention)
- P2: 31.0s (was 48.6s)
- Wall time: 61.6s ≈ S2_hard time

**Key insight**: The segmented P2 has a double benefit:
1. Direct speedup: P2 itself is faster (sequential cache-friendly vs parallel scattered)
2. Indirect speedup: No rayon thread contention with S2_easy/S2_hard — all 24 threads
   available for the parallel S2 computation from the start

---

## Opt 3 — ValidM List + Alpha Re-tune

**What**: Precompute a filtered list of valid m values for S2_hard Type 1 leaves,
reducing inner loop iteration by ~8.5×. Re-tune alpha curve for the new cost profile.

### The Problem

The Type 1 inner loop in S2_hard iterates over ALL m in [min_m, max_m] (up to 23M values),
checking `mu[m] != 0 && lpf[m] > prime` for each. At 1Q with 648 b values across all
segments: ~15 billion iterations, most rejected. This dominates S2_hard at ~5ns/iteration.

### ValidM List

Pre-filter once: build a sorted list of m values where:
1. `mu[m] != 0` (m is squarefree — eliminates 39% of values)
2. `lpf[m] > primes[c+1]` (lpf exceeds minimum Type 1 threshold — eliminates another ~55%)

Combined: list has ~14% of all m ≤ y (2.7M entries at 1Q vs 23M).
Packed as 8-byte structs (m:u32, lpf:u16, mu:i8), the list is ~21MB — fits in L3 cache.

In the Type 1 loop, binary search (`partition_point`) finds the m range, then iterate
only valid entries. The lpf check against prime[b] still runs per-entry but hits ~55%
(entries already pre-filtered by minimum threshold).

**Iteration reduction**: 15B → ~1.75B iterations = **8.5× fewer**

### Alpha Re-tune

The ValidM optimization makes Type 1 leaves cheaper per unit, shifting the optimal balance.
Re-swept alpha at 1Q and Max i64:

| Alpha | 1Q time | Max i64 time |
|-------|---------|-------------|
| 18 | 63.2s | — |
| 20 | **51.6s** | — |
| 21 | **51.9s** | — |
| 23 | 53.4s | — |
| 24 | — | 360.2s |
| 28 | — | **351.2s** |
| 30 | — | 359.4s |

Updated alpha curve:
- log_x 17-18: `16.0 + 5.0 * (log_x - 17)` → ramps 16→21 (was 16→23)
- log_x > 18: `21.0 + 7.0 * (log_x - 18)` → at Max i64: alpha=27.7 (similar to before)

### Results (Opt 3 vs Opt 2)

| Range | V6 Opt 2 | V6 Opt 3 | Speedup |
|-------|----------|----------|---------|
| 1 Billion | 0.001s | 0.001s | same |
| 100 Trillion | 0.089s | 0.089s | same |
| 10 Quadrillion | 2.71s | 2.31s | 1.17× |
| 100 Quadrillion | 14.30s | 13.95s | 1.03× |
| **1 Quintillion** | **61.6s** | **52.0s** | **1.18×** |
| **Max i64** | **400.8s** | **368.1s** | **1.09×** |

**Profiling at 1 Quintillion** (Opt 3):
- S1: 0.04s
- S2_easy: 51.0s (now balanced with S2_hard!)
- S2_hard: 51.1s (was 60.7s — ValidM saves ~10s)
- P2: 32.3s
- Wall time: 52.0s

### Cumulative Optimization Progress

| Range | V6 Opt 0 | V6 Opt 3 | Total Speedup |
|-------|----------|----------|---------------|
| 1 Quintillion | 113.4s | 52.0s | **2.18×** |
| Max i64 | 547.3s | 368.1s | **1.49×** |

---

## Opt 4 — P2 Pre-sieve Masks for Primes 3, 5, 7

**What**: Replace per-bit sieve crossings for small primes (3, 5, 7) in the P2
segmented sieve with word-aligned bitmask operations — 64× fewer operations.

### Implementation

Pre-compute p masks for each prime p ∈ {3, 5, 7}, where mask[offset] has bits set
at positions offset, offset+p, offset+2p, ... within a u64 word.

For each segment, compute the starting alignment and apply all three masks per word:
```
for w in 0..nwords {
    bitmap[w] &= !(masks3[o3] | masks5[o5] | masks7[o7]);
    o3 = (o3 + 2) % 3;  // step derived from 64 mod p
    o5 = (o5 + 1) % 5;
    o7 = (o7 + 6) % 7;
}
```

First segment: restore bits for primes 3, 5, 7 after mask application.
Remaining cross primes (≥ 11) use the existing 4x-unrolled per-bit loop.

### Results (Opt 4 vs Opt 3)

| Range | Opt 3 | Opt 4 | P2 Speedup |
|-------|-------|-------|------------|
| 100 Trillion | 0.331s | 0.217s | 1.53× |
| 1 Quadrillion | 0.806s | 0.518s | 1.56× |
| 10 Quadrillion | 2.31s | 2.32s | same (S2 bottleneck) |
| 100 Quadrillion | 13.95s | 14.38s | same (S2 bottleneck) |
| 1 Quintillion | 52.0s | 51.6s | same (P2: 32→23s) |
| **Max i64** | **368.1s** | **350.2s** | **1.05×** (P2: 156→98s) |

P2 itself is 26-35% faster across all scales. At Max i64, the reduced P2 thread
contention allows S2_hard to benefit from freed CPU resources: 365s → 347s.

### Cumulative Progress

| Range | V6 Opt 0 | V6 Opt 4 | Total Speedup |
|-------|----------|----------|---------------|
| 100 Trillion | 0.089s | 0.217s | 0.41× (direct path overhead) |
| 1 Quintillion | 113.4s | 51.6s | **2.20×** |
| Max i64 | 547.3s | 350.2s | **1.56×** |

---

## Opt 5: Alpha Curve Re-tune for Max i64

### Problem

With V6 Opt 4, the alpha curve for log_x > 18 used slope 7.0, giving alpha ≈ 27.8
at Max i64 (log_x ≈ 18.965). Profiling showed S2_easy and S2_hard unbalanced at
certain scales, and an alpha sweep at Max i64 revealed:

| Alpha | Max i64 Wall |
|-------|-------------|
| 21 | 408.4s |
| 23 | 354.6s |
| 25 | 374.5s |
| 27 | 367.2s |
| 29 | 343.2s |
| 31 | **339.5s** |
| 32 | 350.9s |
| 33 | 352.3s |

Alpha=31 is the sweet spot — S2_easy and S2_hard perfectly balanced at ~339s each.

### Fix

Changed the log_x > 18 slope from 7.0 to 10.4:
- Old: `21.0 + 7.0 * (log_x - 18.0)` → alpha=27.8 at Max i64
- New: `21.0 + 10.4 * (log_x - 18.0)` → alpha=31.0 at Max i64

The alpha at 1Q (log_x=18.0) remains 21.0 — only the ramp above 1Q changes.

### Results (Opt 5 vs Opt 4)

| Range | Opt 4 | Opt 5 | Improvement |
|-------|-------|-------|-------------|
| 1 Quintillion | 51.6s | 51.8s | same |
| **Max i64** | **362.8s** | **342.5s** | **5.6%** |

### Cumulative Progress (V6 Opt 0 → V6 Opt 5)

| Range | V6 Opt 0 | V6 Opt 5 | Total Speedup |
|-------|----------|----------|---------------|
| 100 Trillion | 0.089s | 0.22s | 0.40× (direct path overhead) |
| 1 Quintillion | 113.4s | 51.8s | **2.19×** |
| Max i64 | 547.3s | 342.5s | **1.60×** |

### Failed Experiment: Word-Mask Sieve Crossing

Also tested word-level bitmask crossing for small primes (p < 32) in S2_hard/S2_easy.
The idea: precompute per-word masks for primes 17, 19, 23, 29, 31 and clear all
composites per word at once (popcount + mask) instead of bit-by-bit.

Result: **no measurable improvement**. The sieve crossing for small primes accounts
for < 1% of S2_hard's total time — the m-loop and sieve counting dominate. The
word-level approach also touched every word sequentially vs the 4x-unrolled stride
pattern, adding cache pressure without benefit. Reverted.
