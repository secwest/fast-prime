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

## Next Optimization Targets

### Bottleneck Analysis (Opt 3, Max i64)

- **B = 97.7s** — builds 24GB BigPiTable covering [0, 232B], then parallel lookups. Construction is ~95% of time (parallel segmented sieve of 232B odd numbers with ~40K small primes per segment). Could benefit from pre-sieve masks (3, 5, 7) to reduce crossing work.
- **AC = 87.7s** — C2+A parallel loops with BigPiTable(3B) lookups into 285MB table. Random access pattern. Could benefit from V6-style L2-segmented pi processing.
- **D = 87.7s** — Parallel sieve with ValidM list. Already well-optimized. Could try compressed FactorTable (2 bytes vs 3+ bytes per entry).

### Planned Optimizations

1. **Pre-sieve masks for BigPiTable** — Word-level bitmask clearing for primes 3, 5, 7 during BigPiTable construction. 64× fewer operations for these three primes. Expected 20-30% speedup on B construction.
2. **Alpha fine-tuning** — Sweep individual table entries at Max i64 and 1Q for local optima. Current table values may not be fully optimal.
3. **Compressed FactorTable** — Pack mu (2 bits) + lpf_idx (14 bits) into 2 bytes, halving memory for D's table access. Better L3 cache utilization.
4. **AC segmented pi** — V6-style L2-segmented processing for AC's BigPiTable lookups. Currently 285MB table causes random DRAM access.
5. **Load-balanced D** — Work-stealing for uneven D chunk sizes.
6. **Extended pre-sieve** — Add primes 11, 13 to pre-sieve template for D and BigPiTable sieves.
7. **Segment size tuning** — Sweep segment sizes for BigPiTable, D, and AC.
