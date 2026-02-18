# Optimization Log

Every optimization attempted during development of the fast-prime sieve,
with measured results on an **Intel Core Ultra 9 285K** (Arrow Lake, 8 P-cores + 16 E-cores, 24 threads).

All times are best-of-3 runs unless noted. Hardware: Arrow Lake, DDR5, Windows, Rust 1.x release mode with `lto = "fat"`, `codegen-units = 1`, `target-cpu=native`.

---

## Baseline: Odds-Only Segmented Sieve (Reference Code)

The starting point — a straightforward odds-only segmented sieve using `primal` for bootstrap, `rayon` for parallelism, 512KB segments, one byte per odd candidate.

| Range | Time |
|---|---|
| 1 Billion | 0.107s |
| 10 Billion | 1.119s |
| 100 Billion | 11.609s |
| 1 Trillion | 119.618s |

---

## ✅ Optimization 1: Wheel Mod 30

**What:** Replace odds-only (2 residues/byte) with wheel mod 30 (8 residues coprime to 2×3×5 per byte). Each byte covers 30 numbers instead of 16.

**Why it helps:** Reduces candidate count by ~73% vs odds-only. Same memory covers 1.875× more numbers.

**Result:** ~6–7× faster than baseline across all sizes.

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Billion | 0.107s | 0.018s | 5.9× |
| 10 Billion | 1.119s | 0.158s | 7.1× |
| 100 Billion | 11.609s | 1.766s | 6.6× |
| 1 Trillion | 119.618s | 20.971s | 5.7× |

---

## ✅ Optimization 2: 4× Unrolled Inner Sieve Loop

**What:** Unroll the sieve marking loop from 1 write per iteration to 4.

**Why it helps:** Reduces loop overhead (branch, increment, compare) by 4×. Better instruction-level parallelism.

**Result:** Included in the wheel30 implementation from the start. Contributes ~15-20% vs non-unrolled.

---

## ✅ Optimization 3: Adaptive Segment Sizing

**What:** Instead of fixed segment size, dynamically choose segment size to ensure `num_threads × 8` work units minimum. Range: 8KB–768KB.

**Why it helps:** For small ranges (1B), fixed 512KB segments create only ~65 segments for 24 threads. Smaller segments provide better work-stealing granularity.

**Result:** Significant improvement at small sizes (1B).

| Range | Fixed 512KB | Adaptive |
|---|---|---|
| 1 Billion | 0.018s | 0.015s |
| 10 Billion | 0.158s | 0.158s |

---

## ✅ Optimization 4: Pre-sieve Pattern for Primes 7, 11, 13

**What:** Pre-compute a 1001-byte repeating pattern (LCM of 7×11×13) with all composites of these primes already marked. Tile it across each segment via `copy_from_slice` (memcpy) instead of running the inner loop.

**Why it helps:** Primes 7, 11, 13 are the most prolific sieving primes (highest hit density). Replacing scatter writes with bulk memcpy is much faster.

**Result:** ~5-10% improvement.

⚠️ **Bug encountered:** Initially built the pattern with `seg_start=0` and a `k < 2` guard, which caused alignment mismatches when tiled to other segment positions. Fix: mark ALL multiples including k=1, since primes 7/11/13 never appear in sieve segments (which start > √N).

---

## ✅ Optimization 5: Precomputed TARGET_K_MOD Table

**What:** Replace per-residue `mod_inv_30()` call in `compute_starts` with a compile-time lookup table `TARGET_K_MOD[p_idx][residue_idx]`.

**Why it helps:** Eliminates 8 function calls per prime per segment. The table is 8×8 = 64 entries, fits in a cache line.

**Result:** ~2-3% improvement.

---

## ✅ Optimization 6: Hoisted k_min/k_rem Computation

**What:** Move `k_min = ceil(seg_start/p)` and `k_rem = k_min % 30` out of the per-residue loop.

**Why it helps:** These values are the same for all 8 residues of a given prime. Eliminates 7 redundant divisions.

**Result:** Small but measurable (~1%).

---

## ✅ Optimization 7: u32 Sieving Primes (Half Memory Bandwidth)

**What:** Store sieving primes as `u32` instead of `u64`. Primes ≤ √(10^12) ≈ 10^6 fit in u32.

**Why it helps:** Halves the memory bandwidth when iterating the prime list (~78K primes at 1T).

**Result:** ~1-2% improvement at 1T.

---

## ✅ Optimization 8: 768KB L2 Segments

**What:** Increase MAX_SEG_BYTES from 512KB to 768KB.

**Why it helps:** Arrow Lake P-cores have 2MB L2. 768KB leaves room for the sieving prime list and other working data while maximizing segment size (fewer compute_starts calls).

**Result:**

| Range | 512KB | 768KB |
|---|---|---|
| 100 Billion | 1.60s | 1.59s |
| 1 Trillion | 19.15s | 18.21s |

768KB was the sweet spot. 1MB was slightly better at 1T but worse at smaller sizes. 2MB was significantly worse (L2 cache thrashing).

---

## ✅ Optimization 9: Small/Large Prime Split

**What:** Split sieving primes into small (p < seg_bytes, 4× unrolled) and large (p ≥ seg_bytes, single-write per residue).

**Why it helps:** Large primes hit at most once per segment per residue. The unrolled loop condition check is wasted overhead for them.

**Result:** Marginal (~0.5%) but clean code separation.

---

## ✅ Optimization 10: Barrett Reciprocal Fast Division

**What:** Precompute `recip = ceil(2^64 / p)` for each sieving prime. Replace `k_min = ceil(seg_start / p)` (u64 division) with `mulhi(seg_start + p - 1, recip)` (128-bit multiply).

**Why it helps:** u64 division is ~30-40 cycles on modern CPUs. 128-bit multiply is ~4-6 cycles. Called ~78K × 43K = 3.4 billion times at 1T.

**Result:**

| Range | Before | After |
|---|---|---|
| 1 Trillion | 18.21s | 17.97s |

~1.3% improvement. Small but free.

---

## ✅ Optimization 11: Extended Pre-sieve to 7×11×13×17×19

**What:** Extend the pre-sieve pattern from primes {7, 11, 13} (1001 bytes) to {7, 11, 13, 17, 19} (323,323 bytes).

**Why it helps:** Eliminates inner-loop sieving for two more small primes. At 768KB segments, the 323KB pattern tiles ~2.4 times — still efficient.

**Result:**

| Range | 7×11×13 | 7×11×13×17×19 |
|---|---|---|
| 100 Billion | 1.60s | 1.56s |
| 1 Trillion | 18.0s | 17.8s |

Adding prime 23 (period 7.4MB) was worse at small sizes due to pattern build time.

---

## ✅ Optimization 12: OnceLock Lazy Pre-sieve

**What:** Build the 323KB pre-sieve pattern once using `std::sync::OnceLock` instead of rebuilding per call.

**Why it helps:** When `count_primes` is called multiple times (6 test cases), the pattern is built only on first call (~0.4ms saved per subsequent call).

**Result:** Small sizes (1K, 1M) drop from ~0.4ms to ~0.06ms.

---

## ✅ Optimization 13: 8KB Minimum Segment Size

**What:** Reduce MIN_SEG_BYTES from 16KB to 8KB.

**Why it helps:** For small ranges, allows more fine-grained work-stealing. At 1B, creates more segments for 24 threads.

**Result:**

| Range | 16KB min | 8KB min |
|---|---|---|
| 1 Billion | 0.014s | 0.013s |
| 1 Trillion | 17.8s | 17.5s |

---

## ✅ Optimization 14: L1 Sub-Segmentation (THE BIG ONE — ~2× speedup)

**What:** For tiny primes (p < L1_SEG_BYTES/4 ≈ 6144), process each 768KB L2 segment in 24KB sub-segments that fit entirely in L1 data cache. All tiny primes are processed on one 24KB sub-segment before moving to the next.

**Why it helps:** Tiny primes (p < 6144, ~780 primes) dominate sieve work — each writes 768K/p × 8 ≈ 100K–360K bytes per segment. Without sub-segmentation, each prime's inner loop walks the entire 768KB segment with stride p, causing L1 misses on nearly every write. With 24KB sub-segments, the sieve data stays hot in L1 (Arrow Lake has 48KB L1d), and all tiny primes write to the same cached data before moving on.

**Result:**

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Billion | 0.013s | 0.007s | 1.86× |
| 10 Billion | 0.145s | 0.064s | 2.27× |
| 100 Billion | 1.557s | 0.721s | 2.16× |
| 1 Trillion | 17.50s | 9.30s | 1.88× |

### L1 Sub-Segment Size Tuning

| L1 Sub-Seg Size | 100B | 1T |
|---|---|---|
| 16KB | 0.824s | 10.11s |
| 20KB | 0.789s | 9.84s |
| 24KB | **0.720s** | **9.29s** |
| 32KB | 0.806s | 9.98s |
| 48KB | 0.988s | 12.17s |

24KB (half of L1d) was optimal — leaving headroom for sieve prime data and stack.

### Tiny Threshold Tuning

| Threshold | 100B | 1T |
|---|---|---|
| L1/8 = 3072 | 0.766s | 9.58s |
| L1/4 = 6144 | **0.720s** | **9.29s** |
| L1/2 = 12288 | 0.767s | 9.76s |
| L1 = 24576 | 0.928s | 11.39s |

L1/4 was optimal — primes with ≥4 hits per sub-segment benefit from L1 locality. Expanding further adds compute_starts overhead (called per sub-segment per prime).

---

## ✅ Optimization 15: 3-Tier Medium Prime Split

**What:** Add a "medium" tier (seg_bytes/8 ≤ p < seg_bytes) with a simple non-unrolled loop, between small (4× unrolled) and large (single-write).

**Why it helps:** Medium primes iterate the inner loop only 1-8 times. The 4× unroll check `idx + 3*p < len` is wasted for them.

**Result:** Marginal (~0.5%). Kept for code clarity.

---

## ❌ Failed Optimization: AVX2 VPSHUFB Popcount

**What:** Replace `count_ones()` in counting phase with AVX2 VPSHUFB-based popcount on 32 bytes at a time.

**Why it failed:** The counting phase is already fast (< 1% of total time). The bottleneck is sieve marking (scatter writes), not counting. Hardware POPCNT via `count_ones()` is already optimal.

**Result:** No measurable improvement.

---

## ❌ Failed Optimization: P-Core Only (8 threads)

**What:** Restrict to 8 threads (P-cores only) to avoid slow E-cores.

**Why it failed:** Even though E-cores are slower per-core, the aggregate throughput of 16 E-cores adds ~80% more compute. Rayon work-stealing naturally gives E-cores smaller work units.

**Result:**

| Range | 24 threads | 8 threads (P-only) |
|---|---|---|
| 1 Trillion | 20.97s | 55.3s |

**2.7× slower** with P-cores only.

---

## ❌ Failed Optimization: Cross-Hatch Sieve (Interleaved Residues)

**What:** Instead of 8 separate stride-p passes per prime, interleave all 8 residue hits in sorted byte order within a single forward pass.

**Why it failed:** The sorting/setup overhead per prime per segment negated the cache locality benefit. The 8 separate passes are already efficient because each pass has excellent spatial locality (sequential stride-p writes).

**Result:** Attempted twice. First attempt added ~5% overhead from sorting. Second attempt (precomputed wheel schedule) was equally complex with no gain.

---

## ❌ Failed Optimization: Per-Prime OR Pattern Tiling

**What:** For each small prime p, build a p-byte pattern and tile it across the segment with an OR loop (read-modify-write every byte) instead of scatter writes.

**Why it failed:** For prime p=17, scatter writes touch 768K/17 × 8 ≈ 360K bytes. OR tiling reads+writes ALL 768K bytes. The OR approach uses ~2× more memory bandwidth.

**Result:** ~20% slower for small primes.

---

## ❌ Failed Optimization: 8× Unrolled Inner Loop

**What:** Increase inner loop unrolling from 4× to 8×.

**Why it failed:** Extra instructions and register pressure outweigh the reduced loop overhead. The CPU's out-of-order engine already handles the 4× unrolled loop efficiently.

**Result:** ~1% slower.

---

## ❌ Failed Optimization: Adding Prime 23 to Pre-sieve

**What:** Extend pre-sieve pattern to 7×11×13×17×19×23 (7.4MB period).

**Why it failed:** The 7.4MB pattern exceeds L1 and most of L2. Pattern build time (~11ms) dominates small test cases. Tiling a 7.4MB pattern is less efficient than sieving prime 23 directly.

**Result:** 1T slightly better (17.71s vs 17.84s) but 1K/1M cases 30× slower. Not worth it.

---

## ❌ Failed Optimization: 2MB Segments

**What:** Increase MAX_SEG_BYTES to 2MB.

**Why it failed:** Exceeds Arrow Lake P-core L2 (2MB per core). Sieve data competes with sieving prime list and other working data for L2 space.

**Result:**

| Range | 768KB | 2MB |
|---|---|---|
| 100 Billion | 1.59s | 2.33s |
| 1 Trillion | 18.21s | 24.38s |

**34% slower** at 1T.

---

## ❌ Failed Optimization: Carried-Forward State for Large Primes

**What:** Partition segments into contiguous ranges per thread. Carry `next_hit` absolute positions across segments to avoid recomputing `compute_starts` for large primes.

**Why it failed:** Complex bookkeeping for segment transitions. A bug caused overcounting by ~12M at 1T (large primes missing some segment transitions). Even if correct, the fixed partitioning loses rayon's work-stealing benefit for heterogeneous cores.

**Result:** Abandoned due to correctness issues.

---

## Summary: Cumulative Improvement

| Range | Ref. Baseline | Final | Total Speedup |
|---|---|---|---|
| 1 Billion | 0.107s | 0.007s | **15.3×** |
| 10 Billion | 1.119s | 0.064s | **17.5×** |
| 100 Billion | 11.609s | 0.721s | **16.1×** |
| 1 Trillion | 119.618s | 9.300s | **12.9×** |

The single biggest optimization was L1 sub-segmentation (#14), providing ~2× speedup. Wheel mod 30 (#1) provided the foundational ~6× improvement over the odds-only baseline.
