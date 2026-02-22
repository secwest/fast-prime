# V7 Optimization Log — Gourdon's Algorithm

## Architecture

V7 implements Gourdon's 2001 algorithm, a refinement of the Deleglise-Rivat method used in V5/V6.

**Formula**: π(x) = AC - B + D + Φ₀ + Σ

**Key difference from V6 (Deleglise-Rivat)**:
- **Two tuning parameters**: α_y (for y = x^{1/3}·α_y) and α_z (for z = y·α_z)
- **x\* = max(x^{1/4}, ⌈x/y²⌉)** — defines boundary between formula domains
- **D (hard leaves)** processes fewer leaves than V6's S2_hard via tighter x\* bounds
- **Σ (Sigma)** — 7 cheap arithmetic correction formulas
- **AC (merged)** — replaces V6's S2_easy, uses BigPiTable for O(1) π lookups
- **B** — sum of π(x/p) for primes y < p ≤ √x, equivalent to V6's P2

### Components

| Component | Description | Complexity |
|-----------|-------------|------------|
| Σ (Sigma) | 7 arithmetic formulas (Σ₀-Σ₆) | O(x^{1/3}) |
| Φ₀ | Recursive Möbius sum over squarefree n ≤ z | O(z) |
| B | Σ π(x/p) for primes in (y, √x] | O(x^{2/3}/ln x) |
| AC | C1 (recursive) + C2 (clustered) + A (easy) | O(x^{2/3}/ln x) |
| D | Hard leaves via segmented sieve | O(x^{2/3}/ln x) |

### Five Bugs Fixed During Initial Implementation

1. **mu[p] computation** — iterate from p (not 2p) in generate_tables(). Missing p caused mu[p²]=0 to not propagate.
2. **C2 lower bound** — must be max(k, pi_root3_xy, pi_sqrtz) + 1, not just k+1. Missing constraints caused double-counting of some leaves.
3. **Pi table size** — must extend to max(z, max_a_prime), not just z. Sigma's Σ₆ accesses pi[√(x/p)] which can exceed z.
4. **cross_off_sieve p² special case** — the optimization "if p > sqrt(high), only remove p itself" is correct for PRIME sieves but WRONG for phi counting sieves. Phi sieves need ALL odd multiples removed (3p, 5p, etc.), not just p². This caused systematic undercounting in parallel D at 100B+.
5. **Parallel D cur_max_b look-ahead** — the parallel D had `pi[isqrt(remaining_chunk_end)]` limiting which b values got phi updates. This prevented phi accumulation for some b values, so later chunks got wrong prefix_phi. Fixed by matching the serial formula exactly: `min(pi[isqrt(x/low1)], pi_x_star)`.

---

## Opt 0 — Baseline: Gourdon's Algorithm

**What**: Initial correct implementation of Gourdon with parallel D, concurrent B/AC/D.

**Implementation**:
- `compute_sigma()`: 7 formulas, O(x^{1/3}) — negligible time
- `compute_phi0()`: Recursive Möbius, parallel over first prime via rayon
- `compute_b()`: Serial segmented sieve building π(n) up to x/y, then Σ π(x/p)
- `compute_ac()`: C1 recursive (sequential), C2+A parallel over b (rayon), using BigPiTable for O(1) π lookups
- `compute_d()`: Parallel via chunk-based phi correction (same design as V4/V5/V6's parallel S2)
- `count_primes()`: B, AC, D run concurrently via `thread::scope`
- Alpha: Walisch's polynomial fit — α_yz = 0.00527·ln³(x) - 0.4955·ln²(x) + 16.58·ln(x) - 183.8, α_z = min(α_yz/5, 2)

**Results (V7 Opt 0 vs V6 Opt 5)**:

| Range | V6 | V7 | Speedup |
|-------|-----|-----|---------|
| 1 Trillion | 0.013s | 0.008s | 1.7× |
| 100 Trillion | 0.217s | 0.108s | 2.0× |
| 100 Quadrillion | 14.379s | 7.053s | 2.0× |
| **1 Quintillion** | **51.8s** | **32.4s** | **1.6×** |
| **Max i64** | **342.5s** | **202.1s** | **1.7×** |

**Profile (V7 Opt 0)**:

| Component | 100Q | 1 Quint | Max i64 |
|-----------|------|---------|---------|
| Tables | 0.25s | 0.91s | 2.72s |
| B | 6.83s | 31.9s | 183.6s |
| AC | 0.89s | 9.84s | 184.8s |
| D | 6.28s | 28.8s | 197.9s |
| **Wall** | **7.1s** | **32.9s** | **200.9s** |

**Bottleneck**: B is single-threaded (serial segmented sieve), taking 31.9s at 1Q while D+AC take ~30s with 24 threads. At Max i64, all three are comparable (~185-198s) because D dominates.

---

## Opt 1 — Parallel B via BigPiTable

**What**: Replace 150-line serial segmented sieve B with 20-line BigPiTable-based parallel implementation.

### The Problem

B computes Σ π(x/p) for primes y < p ≤ √x. The old approach built a full segmented sieve covering [0, x/y], computed π incrementally as segments were processed, and accumulated the sum. This was single-threaded, taking 31.9s at 1Q — the wall-time bottleneck.

### The Solution

1. Build a `BigPiTable` covering [0, max(x/p)] = [0, x/smallest_B_prime] using parallel segmented sieve
2. Look up π(x/p) for each prime via rayon `par_iter` — O(1) per lookup
3. Run B concurrently with D+AC via `thread::scope` (all three share rayon pool)

**Critical fix**: BigPiTable prefix sums changed from `Vec<u32>` to `Vec<u64>` — at Max i64, max_xp ≈ 232B which has π > 4.3 billion, exceeding u32::MAX.

**Memory**: ~12 bytes per 128 odd numbers:
- 1Q (max_xp ≈ 73.5B): ~6.9GB
- Max i64 (max_xp ≈ 232B): ~24.2GB

### Results (Opt 1 vs Opt 0)

| Range | Opt 0 | Opt 1 | Speedup |
|-------|-------|-------|---------|
| 100 Quadrillion | 7.05s | 6.68s | 1.06× |
| **1 Quintillion** | **32.9s** | **29.0s** | **1.13×** |
| Max i64 | ~200s | ~200s | same |

At 1Q: 12% faster — B drops from 31.9s to ~7s (hidden behind D at 28.8s).
At Max i64: unchanged — B's BigPiTable construction (~90s) is comparable to D/AC.

---

## Opt 2 — ValidM List for D Type 1

**What**: Pre-compute filtered list of valid m values for D Type 1 leaves, reducing inner-loop iterations by ~8.5×.

### The Problem

The Type 1 inner loop in D iterates ALL m in [min_m, max_m] (up to 23M values at 1Q), checking:
- `mu[m] != 0` (squarefree — eliminates ~39%)
- `lpf[m] > prime` (least prime factor constraint)
- `y_smooth[m]` (all prime factors ≤ y)

At 1Q with 648 b values across all segments: ~15 billion iterations, most rejected at ~5ns each.

### The Solution

Pre-filter once: build a sorted list of m values where:
1. `mu[m] != 0` (m is squarefree)
2. `lpf[m] > primes[c+1]` (lpf exceeds minimum Type 1 threshold)
3. `y_smooth[m]` (all prime factors ≤ y)

Combined: list has ~14% of all m ≤ y (2.7M entries at 1Q vs 23M). Packed as 8-byte structs (m:u32, lpf:u16, mu:i8), ~21MB — fits in L3 cache.

In the Type 1 loop, binary search (`partition_point`) finds the m range per (b, segment), then iterate only valid entries. The lpf check against prime[b] still runs per-entry but hits ~55% of entries (already pre-filtered by minimum threshold).

**Iteration reduction**: 15B → ~1.75B = **8.5× fewer**

### Results (Opt 2 vs Opt 1)

| Range | Opt 1 | Opt 2 | Speedup |
|-------|-------|-------|---------|
| 100 Quadrillion | 6.68s | 2.76s | 2.4× |
| **1 Quintillion** | **29.0s** | **16.4s** | **1.8×** |
| Max i64 | ~200s | ~200s | same |

At 100Q: **2.4× faster** — D was the bottleneck, now much cheaper.
At 1Q: **1.8× faster** — D drops from bottleneck to balanced.
At Max i64: unchanged — D is no longer the bottleneck at this scale.

---

## Opt 3 — Alpha Lookup Table

**What**: Replace Walisch's polynomial alpha curve with a piecewise-linear interpolation table, tuned for the ValidM-optimized Gourdon implementation.

### The Problem

The polynomial alpha curve `α_yz = 0.00527·ln³(x) - 0.4955·ln²(x) + 16.58·ln(x) - 183.8` was designed for a different cost profile (Walisch's C++ primecount). After Opt 1 (parallel B) and Opt 2 (ValidM) changed the relative costs of B/AC/D, the optimal alpha values shifted significantly.

Additionally, Gourdon's algorithm has TWO alpha parameters (α_y and α_z) that should be tuned independently, not derived from a single α_yz.

### The Solution

Replace the polynomial with a 9-point lookup table with linear interpolation between entries. Each entry specifies (logx, α_y, α_z) independently:

| logx | α_y | α_z | ~x |
|------|-----|-----|-----|
| 20.0 | 2.0 | 1.5 | 5×10⁸ |
| 23.0 | 3.0 | 1.5 | 10¹⁰ |
| 25.3 | 4.0 | 2.0 | 10¹¹ |
| 30.0 | 6.0 | 2.0 | 10¹³ |
| 34.5 | 8.0 | 2.0 | 10¹⁵ |
| 36.8 | 10.0 | 2.0 | 10¹⁶ |
| 39.1 | 14.0 | 2.5 | 10¹⁷ |
| 41.4 | 15.0 | 3.5 | 10¹⁸ |
| 43.7 | 19.0 | 4.5 | Max i64 |

Environment variable override (`ALPHA_Y`, `ALPHA_Z`) retained for sweep experiments.

### Results (Opt 3 vs Opt 2)

| Range | Opt 2 | Opt 3 | Speedup |
|-------|-------|-------|---------|
| 100 Quadrillion | 2.76s | 2.74s | same |
| **1 Quintillion** | **16.4s** | **10.8s** | **1.52×** |
| **Max i64** | **~200s** | **95.4s** | **2.1×** |

**Massive improvement at large scales**: the new alpha table pushes α_y=19, α_z=4.5 at Max i64 (vs ~15, 3.5 before), which increases y (reducing B range) and z (reducing D range) simultaneously.

### Profile (Opt 3)

| Component | 100Q | 1 Quint | Max i64 |
|-----------|------|---------|---------|
| Tables | 0.03s | 0.09s | 0.24s |
| B | 2.24s | 7.46s | 97.7s |
| AC | 1.32s | 8.03s | 87.7s |
| D | 2.08s | 9.37s | 87.7s |
| **Wall** | **2.73s** | **11.4s** | **104.8s** |

At Max i64: B (97.7s) is now the clear bottleneck. AC (87.7s) and D (87.7s) are perfectly balanced.

### Cumulative Optimization Progress

| Range | V7 Opt 0 | V7 Opt 3 | Speedup | vs V6 |
|-------|----------|----------|---------|-------|
| 100Q | 7.05s | 2.74s | 2.6× | 5.3× vs V6 |
| 1 Quintillion | 32.9s | 10.8s | 3.0× | 4.8× vs V6 |
| Max i64 | 200.9s | 95.4s | 2.1× | 3.6× vs V6 |

---

## Opt 4 — Pre-sieve Masks for BigPiTable + Alpha Table Fix

**What**: Two changes that together reduce Max i64 from 95.4s to **62.2s** (35% faster).

### Pre-sieve Masks for BigPiTable Construction

Add word-level bitmask pre-sieve for primes 3, 5, 7 in BigPiTable parallel sieve construction. Instead of clearing composites bit-by-bit, precompute AND-masks for each prime's offset pattern and apply all three per word in a single pass. Remaining primes (≥ 11) use existing per-bit loop.

For prime p in odd-index space, odd multiples are at indices ≡ (p-1)/2 (mod p). Each word advances the offset by Δ = (p - 64%p) % p:
- p=3: Δ=2, cycle length 3
- p=5: Δ=1, cycle length 5
- p=7: Δ=6, cycle length 7

First segment: restore prime bits for 3, 5, 7 after mask application.

**Impact**: B at Max i64: 97.7s → 87.0s (11% faster construction). D benefits from reduced rayon thread contention: 87.7s → 75.4s.

### Alpha Table Fix: The az=4.5 Cliff

Exhaustive sweep of α_z at Max i64 (α_y=19 fixed) revealed a dramatic performance cliff:

| α_z | Time | B | AC | D |
|-----|------|---|----|----|
| 4.2 | 98.4s | 88.3s | 88.9s | — |
| 4.3 | 98.3s | — | — | — |
| 4.4 | 97.1s | 88.3s | 88.9s | 77.0s |
| **4.5** | **59.4s** | **39.4s** | **53.5s** | **50.4s** |
| 4.6 | 63.5s | — | — | — |
| 4.8 | 61.8s | — | — | — |

**37% speedup** from changing α_z by just 0.1! Root cause: at α_z ≥ 4.5, D and AC finish fast enough that B gets exclusive rayon pool access for its final construction phase. Below 4.5, all three components compete for threads for their full duration.

**The bug**: the alpha lookup table's last entry was at logx=43.7, but Max i64 has logx=43.68. Linear interpolation gave α_z=4.49 — just below the 4.5 cliff. Fixed by moving the entry to logx=43.6 so Max i64 correctly uses (19.0, 4.5).

### Results (Opt 4 vs Opt 3)

| Range | Opt 3 | Opt 4 | Speedup |
|-------|-------|-------|---------|
| 1 Quintillion | 10.8s | 11.1s | same |
| **Max i64** | **95.4s** | **62.2s** | **1.53×** |

### Cumulative Progress

| Range | V7 Opt 0 | V7 Opt 4 | Speedup | vs V6 |
|-------|----------|----------|---------|-------|
| 100Q | 7.05s | 2.80s | 2.5× | 5.1× |
| 1 Quintillion | 32.9s | 11.1s | 3.0× | 4.7× |
| Max i64 | 200.9s | 62.2s | 3.2× | **5.5×** |

---

## Opt 5 — Software Prefetch + L2-Sized D Segments

**What**: Two changes that reduce Max i64 from 62.2s to **50.1s** (19.5% faster).

### Software Prefetch for AC's BigPiTable Lookups

Add `_mm_prefetch` intrinsic calls to pre-load BigPiTable data for upcoming iterations in AC's hot loops:
- **C2 sparse loop** (4x unrolled): prefetch next iteration's 4 BigPiTable entries (8 cache lines: 4 bits + 4 prefix)
- **A formula loops** (x/pq≥y and x/pq<y): 4x unrolled with prefetch 4 ahead

The AC BigPiTable at Max i64 is 380MB (10× L3 cache). Random lookups hit DRAM at ~100ns. Prefetching one iteration ahead (4 values) hides most of the DRAM latency.

**Impact on AC**: 53.5s → 50.0s (6.5% faster)

### L2-Cache-Sized D Segments

Cap D's segment size at 2^21 = 2M integers (1M odd, 15.6K words, **125KB per sieve**). Previously, D used dynamic sizing that resulted in 67M-integer segments (4.2MB sieve per thread). The key insight:

- Old: 24 threads × 4.2MB = 100MB total sieve memory → exceeds L3 (36MB), causes frequent evictions
- New: 24 threads × 125KB = 3MB total sieve memory → fits in L3 with room for B/AC data

Reducing D's L3 footprint benefits ALL three concurrent components:
- **B**: 38s → 22s (42% faster) — BigPiTable construction gets more L3 bandwidth
- **AC**: 50s → 42s (16% faster) — BigPiTable lookups have fewer L3 evictions
- **D**: 50s → 42s (16% faster) — sieve fits in L2 cache for fast count/count_delta

Tunable via `D_SEG_CAP` env var (default 21, i.e., 2^21). Sweep results at Max i64:
| Cap | Segment | Sieve/thread | Time |
|-----|---------|-------------|------|
| 2^20 | 1M | 62KB | 49.3s |
| **2^21** | **2M** | **125KB** | **48.8s** |
| 2^22 | 4M | 250KB | 50.2s |
| 2^24 | 16M | 1MB | 51.5s |
| 2^26 | 64M | 4.2MB | 62.0s |

### Results (Opt 5 vs Opt 4)

| Range | Opt 4 | Opt 5 | Speedup |
|-------|-------|-------|---------|
| 100Q | 2.80s | 2.76s | same |
| 1 Quintillion | 11.1s | 11.9s | 0.93× |
| **Max i64** | **62.2s** | **50.1s** | **1.24×** |

Note: 1Q shows slight regression because at that scale, larger segments were already near-optimal. The Max i64 improvement dominates.

### Cumulative Progress

| Range | V7 Opt 0 | V7 Opt 5 | Speedup | vs V6 |
|-------|----------|----------|---------|-------|
| 100Q | 7.05s | 2.76s | 2.6× | 5.2× |
| 1 Quintillion | 32.9s | 11.9s | 2.8× | 4.4× |
| Max i64 | 200.9s | 50.1s | 4.0× | **6.8×** |

### Comparison with Kim Walisch's primecount v8.2

primecount is the fastest published prime counting implementation, by Kim Walisch. Both implementations use Gourdon's algorithm.

| Scale | V7 Opt 5 | primecount | Ratio | primesieve |
|-------|----------|------------|-------|------------|
| 1e10 | 0.004s | — | — | 0.054s |
| 1e11 | 0.005s | — | — | 0.565s |
| 1e12 | 0.009s | 0.006s | 1.5× | 6.85s |
| 1e13 | 0.022s | 0.009s | 2.4× | 84s |
| 1e14 | 0.042s | 0.022s | 1.9× | — |
| 1e15 | 0.180s | 0.054s | 3.3× | — |
| 1e16 | 0.686s | 0.174s | 3.9× | — |
| 1e17 | 2.59s | 0.590s | 4.4× | — |
| 1e18 | 11.8s | 2.27s | 5.2× | — |
| Max i64 | 50.5s | 8.40s | 6.0× | — |

Our V7 is ~1.5-6× slower than primecount, with the gap growing at larger scales. primecount has been optimized for over a decade with:
- 128-bit arithmetic for extended range
- Highly tuned segmented sieve and phi tables
- Sophisticated alpha tuning per scale
- Cache-oblivious algorithms
- AVX2/AVX-512 SIMD in some paths

For reference, primesieve (segmented Sieve of Eratosthenes) becomes impractical above 1e12 due to O(n log log n) complexity vs O(n^{2/3+ε}) for Gourdon's algorithm. Our V7 is already 760× faster than primesieve at 1e12.

---

## Next Optimization Targets

### Bottleneck Analysis (Opt 5, Max i64 full suite)

- **AC = 42.4s** — C2+A parallel loops with BigPiTable(3B) lookups. Prefetch helps but table is still 380MB (10× L3).
- **D = 42.5s** — Parallel sieve with L2-sized segments. Near tied with AC.
- **B = 22.2s** — BigPiTable(232B) construction. No longer bottleneck.
- **Setup = 7.0s** — Sieve, pi table, mu/lpf/y_smooth generation (single-threaded).

### Planned Optimizations

1. **AC segmented pi** — V6-style L2-segmented processing for AC's BigPiTable lookups. Process [0,√x] in L2 chunks, narrowing b/l ranges per segment. Highest potential.
2. **Setup overlap** — Start B immediately (it only needs x, y, pi_y), overlap generate_tables with BigPiTable construction. Could save ~5s of wall time.
3. **Compressed FactorTable** — Pack mu (2 bits) + lpf_idx (14 bits) into 2 bytes, halving memory for D's table access.
4. **SIMD popcount for D** — Use AVX2 256-bit popcount for sieve.count_delta() spans.
5. **Alpha scale-dependent D segments** — Use larger D segments at smaller scales where cache pressure is lower.
