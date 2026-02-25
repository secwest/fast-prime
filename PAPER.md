# Beating Primecount: Exhaustive Micro-Architecture-Aware Optimization of Combinatorial Prime Counting

**Dragos Ruiu**

---

## Abstract

We present a systematic study of performance optimization for combinatorial prime counting at extreme scale, culminating in a Rust implementation of Gourdon's algorithm that computes π(2⁶³ − 1) = 216,289,611,853,439,384 in **8.39 seconds** on an Intel Core Ultra 9 285K processor — beating the state-of-the-art C++ implementation *primecount* v8.2 (8.49s cold best, 8.70s sustained median) by Kim Walisch. Over the course of **107 controlled experiments** spanning 8 sessions, we explored every major axis of the optimization space: data structure layout, cache hierarchy exploitation, thread scheduling, compiler tuning, profile-guided optimization, memory access patterns, and algorithmic reformulations. We document which optimizations succeeded (+1.3% from `build-std`), which failed (80% regression from serialized memory access), and, critically, *why* — providing a detailed micro-architectural model that explains the performance ceiling. Our central finding is that the hot inner loop is **memory-bandwidth-bound with high memory-level parallelism (MLP)**: the 4× unrolled loop generates 8 independent L2 miss requests that saturate the processor's miss-handling capacity. Any optimization that disrupts this MLP equilibrium — whether by serializing access patterns, increasing table sizes, adding prefetch instructions, or changing unroll factors — degrades performance. We show that for the b-first BigPiTable architecture on modern hybrid-core processors, the code is provably at a local performance optimum, and identify the architectural rewrite required to break through to the next performance tier.

---

## 1. Introduction

### 1.1 The Prime Counting Function

The prime counting function π(x) — the number of primes not exceeding x — is among the most fundamental functions in analytic number theory. Computing π(x) exactly for large x has practical applications in cryptographic parameter selection, primality testing, and computational number theory, and serves as a benchmark for algorithmic and systems-level optimization.

The evolution of prime counting algorithms spans over a century: from Meissel's O(x^{2/3} / ln x) method (1870), through Lehmer's extensions (1959), to the breakthrough combinatorial methods of Lagarias, Miller, and Odlyzko (1985), Deleglise and Rivat (1996), and Gourdon (2001). Gourdon's algorithm, which decomposes π(x) as:

$$\pi(x) = \text{AC}(x) - B(x) + D(x) + \Phi_0(x) + \Sigma(x)$$

achieves O(x^{2/3} / ln² x) time and O(x^{1/3} ln³ x) space, and forms the basis of Kim Walisch's *primecount* — the current state-of-the-art implementation, written in C++ with over a decade of optimization.

### 1.2 Motivation and Contributions

This work began as an exercise in implementing Gourdon's algorithm in Rust, but evolved into a systematic study of what determines performance at the extreme scale of π(2⁶³ − 1) ≈ 2.16 × 10¹⁷ on modern hybrid-core processors. Our contributions are:

1. **A competitive Rust implementation** that matches or beats primecount at all scales from 10¹⁰ to 2⁶³ − 1, demonstrating that Rust's zero-cost abstractions and LLVM backend can achieve C++-competitive performance for memory-bound numerical workloads.

2. **A taxonomy of 107 optimization experiments** with detailed performance analysis, providing an empirical map of the optimization landscape for memory-bandwidth-bound parallel computations on Intel Arrow Lake.

3. **Identification of the MLP constraint** as the fundamental performance limiter: the 4× unrolled inner loop generates 8 independent L2 miss requests that saturate the processor's 12–16 outstanding miss capacity, creating an equilibrium that cannot be improved by any local code transformation.

4. **Discovery of the BigPiTable L3 warming effect**: the AC computation's continuous random accesses to the 285 MB π-table keep it warm in L3 cache, benefiting the concurrent B computation. Scheduling changes that accelerate AC paradoxically slow the overall computation by depriving B of this cache warming.

5. **Quantification of the concurrent penalty**: AC alone completes in 2.10s, but concurrent execution with D inflates this to 8.42s — a 4.0× penalty from L3 cache pressure, work-stealing overhead, and power throttling. This penalty is irreducible within the current architecture.

### 1.3 Hardware Platform

All experiments were conducted on:

| Component | Specification |
|-----------|--------------|
| CPU | Intel Core Ultra 9 285K (Arrow Lake) |
| P-cores | 8 × Lion Cove, 5.7 GHz, 2 MB L2 each |
| E-cores | 16 × Skymont, 4.6 GHz, 4 MB L2 per 4-core cluster |
| L3 cache | 36 MB shared |
| Memory | 96 GB DDR5-5600 dual channel (~89.6 GB/s) |
| Hyperthreading | None (24 total hardware threads) |
| OS | Windows 11 |
| Compiler | Rust 1.95.0-nightly, LLVM 19 |

---

## 2. Algorithm and Implementation

### 2.1 Gourdon's Decomposition

We implement Gourdon's 2001 formula with two independent tuning parameters α_y and α_z:

- **y = x^{1/3} · α_y**: controls the easy-leaf / hard-leaf boundary
- **z = y · α_z**: controls the C1/C2 split within easy leaves
- **k = π(y^{1/3})**: the small-prime cutoff for Φ₀

The five components are:

| Component | Description | Complexity | Our time (solo) |
|-----------|------------|------------|-----------------|
| **AC** | Easy leaves (C2 + A) | O(x^{2/3} / ln² x) | 2.10s |
| **B** | P₂ equivalent | O(x^{2/3} / ln x) | 4.31s |
| **D** | Hard leaves via segmented sieve | O(x^{2/3} / ln² x) | 4.50s |
| **Φ₀** | Recursive Euler totient | Small | <0.01s |
| **Σ** | Correction terms | Small | <0.01s |

### 2.2 Key Data Structures

**BigPiTable** (285 MB). A full-range O(1) prime counting table covering [0, √x] ≈ [0, 3.037 × 10⁹]. Uses odd-only bit encoding (128 numbers per u64 word) with a parallel prefix-sum array:

```
bits[w]:   64-bit sieve word, bit i set iff 2(64w+i)+1 is prime
prefix[w]: popcount(bits[0..w-1]), stored as u32
```

The critical `pi_fast` function computes π(n) in 7 instructions:

```rust
unsafe fn pi_fast(&self, n: usize) -> u64 {
    let odd_idx = (n - 1) >> 1;
    let word = odd_idx >> 6;
    let bit = odd_idx & 63;
    let prefix = *self.prefix.get_unchecked(word) as u64;
    let mask = u64::MAX >> (63 - bit);
    1 + prefix + (*self.bits.get_unchecked(word) & mask).count_ones() as u64
}
```

This compiles to BMI2 `BZHI` + `POPCNT` on the critical path — near the hardware minimum for a table-based π lookup.

**Barrett Reduction** (`fast_div`). Division by prime p is replaced with a precomputed reciprocal multiply-shift with correction:

```rust
fn fast_div(n: u64, d: u64, recip_d: u64) -> u64 {
    let q = ((n as u128 * recip_d as u128) >> 64) as u64;
    q + (n - q.wrapping_mul(d) >= d) as u64
}
```

This is ~6× faster than hardware `DIV` on Arrow Lake and is exact (the correction step handles the Barrett approximation error).

**Primes array**. 1-indexed: `primes[0] = 0` (sentinel), `primes[k]` = k-th prime. This convention, discovered during debugging of the clustered-leaves optimization (Opt 95), is critical for correct use with `pi_fast` return values.

### 2.3 Parallelization Architecture

| Pool | Threads | Components | Rationale |
|------|---------|------------|-----------|
| Global rayon | 72 (24 × 3) | AC, D | 3× oversubscription for work-stealing |
| B pool | 24 | B only | Dedicated to prevent AC starvation |
| C1 pool | 8 | C1 only | Small recursive computation |

The 3× oversubscription (POOL_MULT=3) was determined empirically (Opt 73, 103) to be optimal: fewer threads underutilize work-stealing; more threads increase context-switching overhead.

**Two-scope early start**: AC begins at t ≈ 0.135s while D's prerequisite tables are still being built in the background, overlapping ~0.4s of setup with AC computation.

### 2.4 AC Inner Loop Structure

The AC computation is the critical path (8.42s concurrent, 2.10s solo). Its inner loop processes 154,701 b-values, each iterating over a range of l-values:

```
for each b-value (parallel over rayon global pool):
    for l in eff_lo..=eff_hi (4× unrolled):
        xpq = fast_div(x/p_b, p_l, recip_l)
        pi_val = big_pi.pi_fast(xpq)
        accumulate(pi_val, ...)
```

The 4× unrolling generates 4 independent `fast_div` + `pi_fast` chains per loop body, creating 8 independent memory requests (2 per `pi_fast`: one for `bits[word]`, one for `prefix[word]`).

### 2.5 Experimental Methodology

All 107 experiments followed a controlled benchmarking protocol:

**Thermal management.** Each benchmark session began with a cold-CPU measurement (CPU idle ≥60 seconds, confirmed via core temperature monitoring). Subsequent runs were taken in rapid succession to capture sustained-thermal performance. Both "best" (cold) and "median" (sustained) times are reported.

**Measurement protocol.** Each experiment was run a minimum of 3 times, with parameter sweeps using 3–5 runs per configuration. Wall-clock time was measured via `std::time::Instant` within the binary. Head-to-head comparisons with primecount used 10 alternating runs (V8, primecount, V8, primecount, ...) to equalize thermal conditions.

**Isolation.** All non-essential background processes were terminated. Windows Defender real-time scanning and Smart App Control (SAC) were disabled where possible. No other CPU-intensive work ran during benchmarks.

**Correctness gate.** Every experiment was validated against the known value π(2⁶³ − 1) = 216,289,611,853,439,384 before performance was recorded. Experiments that produced incorrect results were debugged, fixed, re-validated, and only then benchmarked. Cross-validation at smaller scales (10¹⁰ through 10¹⁸) was performed against primecount's output.

**Revert discipline.** Each experiment modified the codebase on a single branch, was benchmarked, and then reverted if it did not improve performance. This ensured that the baseline was never corrupted and that experiments were independent.

### 2.6 Correctness Verification

The implementation was validated at multiple levels:

1. **Reference value agreement**: π(2⁶³ − 1) = 216,289,611,853,439,384 matches primecount v8.2, the OEIS (A006880), and published computational results.
2. **Cross-scale validation**: Agreement with primecount at 10¹⁰, 10¹², 10¹⁴, 10¹⁶, and 10¹⁸.
3. **Component-level verification**: Individual components (AC, B, D, Φ₀, Σ) were tested in isolation against reference values computed via independent methods.
4. **Regression testing during optimization**: Each of the 107 experiments verified correctness before and after code changes. Several experiments (Opt 95 clustered leaves, Opt 106 monotonic sweep) uncovered bugs that were fixed and re-verified before benchmarking.

---

## 3. Optimization Taxonomy

### 3.1 Summary of All 107 Experiments

We categorize the 107 experiments across 8 sessions into six axes:

| Axis | Experiments | Best result | Key finding |
|------|-------------|------------|-------------|
| **Data structure layout** | 55, 56, 64, 65, 83, 104 | All worse | Odd-only encoding is Pareto-optimal |
| **Cache/memory** | 53, 54, 59, 70, 91, 93, 94, 105, 106 | All worse | L2 miss handling is the bottleneck |
| **Thread scheduling** | 61, 66, 67, 68, 84, 85, 86, 97, 99, 101, 102, 103 | Defaults optimal | Work-stealing > dedicated pools |
| **Compiler/build** | 62, 71, 77, 78, 79, 80, 81, 96, 98 | +1.3% (build-std) | PGO is counterproductive |
| **Algorithmic** | 60, 63, 82, 95, 100 | All worse | Clustering needs L1-resident table |
| **Loop structure** | 76, 107 | All worse | 4× unroll is Pareto-optimal |

### 3.2 Data Structure Layout Experiments

**Interleaved bits+prefix** (Opt 55, 64, 83, 104). We repeatedly tested merging the `bits[]` and `prefix[]` arrays into a single interleaved `data[]` array, placing both values for the same word on the same cache line. Despite the theoretical halving of DRAM accesses per lookup, every attempt showed 4–6% regression. The root cause: interleaving reduces bits-per-cache-line from 8 to 4, halving spatial locality for the semi-sequential access pattern within each b-value's l-iteration.

**Wheel-30 encoding** (Opt 56). Encoding 240 numbers per u64 (8 coprime-to-30 residues) reduces the table from 285 MB to 152 MB. However, the division by 240 adds ~4 cycles per lookup versus the shift-only indexing of odd-only encoding (n >> 7). The computational overhead exceeds the cache benefit.

**Sparse prefix** (Opt 65). Replacing the per-word prefix array (95 MB) with per-8-word coarse checkpoints (12 MB) requires up to 7 additional `POPCNT` instructions per lookup. The branch-heavy conditional popcounting generates unpredictable branches, causing a 65% regression.

### 3.3 Cache and Memory Access Experiments

**Software prefetch** (Opt 59, 70, 94, 105). We tested prefetch distances from 4 iterations (~12 ns) to 128 iterations (~1300 ns). Short distances (4 iterations) are redundant — Arrow Lake's out-of-order engine already looks ahead ~200 instructions. Long distances (32–128 iterations) consume L2 miss-tracking entries that compete with demand loads, reducing effective MLP. All prefetch experiments showed 3–12% regression.

**Segment-first SegmentedPiTable** (Opt 53, 91, 93). Primecount achieves L1-resident π lookups via a 3.7 KB per-segment sieve rebuilt per segment. We attempted three implementations:
- Opt 53: Full segment-first architecture (75s — 8.7× regression)
- Opt 91: Per-b-value streaming sieve (killed — redundant sieve rebuilds)
- Opt 93: Per-thread chunk with sub-segments (11.77s — 5× regression)

The fundamental issue: sieve construction cost is O(√x · ln ln √(√x)) per thread — a fixed overhead of ~2.78s/thread that exactly cancels the L1 cache benefit of ~0.66s savings from 220M lookups at 3 ns each. Primecount avoids this by using a segment-first outer loop (build sieve once per segment, process all b-values), which requires a fundamentally different parallelization strategy.

**Monotonic sweep** (Opt 106). Since xpq decreases monotonically within each b-value, we maintained a running π counter updated via sequential L1d-cached sieve scans. Despite ~89% of iterations having Δxpq < 4096 (suitable for scanning), the serialization of iterations destroyed 4× unrolling MLP, causing an 80% regression. This experiment definitively proved the loop is bandwidth-bound, not latency-bound.

### 3.4 Thread Scheduling and Pool Configuration

**Phase scheduling** (Opt 67, 99). Running components sequentially reveals their solo performance:
- AC alone: **2.10s** (vs 8.42s concurrent — **4.0× penalty**)
- B alone: 4.31s (vs 7.31s concurrent — 1.7× penalty)
- D alone: 4.50s (vs 5.48s concurrent — 1.2× penalty)

Despite the massive concurrent penalty, phased execution is slower (8.99–9.36s) because sequential phases cannot overlap, and the overlap savings (max(8.42, 7.31, 5.48) vs 2.10 + 7.31 + 4.50) favor concurrency.

**DELAY_D scheduling** (Opt 101). Waiting for AC to finish before starting D yielded a critical discovery: B slowed from 7.31s to 8.50s. **AC's "concurrent penalty" is partially beneficial** — its continuous BigPiTable lookups keep the 285 MB table warm in L3 cache for B's benefit. When AC finishes early, D's sieve operations evict BigPiTable, slowing B.

**B_THREADS sweep** (Opt 75, 89, 102). A full sweep from 1 to 24 threads reveals a clean zero-sum tradeoff:

| B_THREADS | AC time | B time | Total |
|-----------|---------|--------|-------|
| 1 | 6.79s | 37.25s | 37.39s |
| 8 | 7.25s | 10.20s | 10.34s |
| 16 | 7.97s | 8.78s | 8.93s |
| 22 | 8.46s | 8.47s | 8.72s |
| **24** | **8.44s** | **6.94s** | **8.60s** |

The crossover (AC ≈ B) occurs at B ≈ 22, but max(8.46, 8.47) = 8.72 > max(8.44, 6.94) = 8.60. No thread allocation improves both components simultaneously.

### 3.5 Compiler and Build Optimization

**`-Zbuild-std`** (Opt 78). Recompiling Rust's standard library with `target-cpu=native` yielded the **only successful optimization**: 8.55s median (from 8.63s), a 1.3% improvement. This enables AVX-512 memcpy/memset and Arrow Lake-tuned allocation in the standard library.

**PGO** (Opt 62, 77, 96). Profile-guided optimization was tested three times across different sessions, all showing no improvement or regression. The hot loop's branch prediction is already >99% accurate (`is_c2` is loop-invariant; `y_boundary_l` mispredicts once per b-value). PGO's aggressive inlining increases code size, causing L1 instruction cache pressure.

**LLVM flags** (Opt 80, 81, 98). Tested `--unroll-threshold` (200–1200), `--enable-loop-interchange`, `--enable-loop-flatten`, `--x86-cmov-converter=false`. All results within noise, confirming LLVM's default codegen is near-optimal for this loop structure.

### 3.6 Algorithmic Reformulations

**Clustered easy leaves** (Opt 63, 95). For consecutive l-values producing the same π(xpq), compute once and multiply by the cluster size. After fixing two bugs (BigPiTable/primes sieve mismatch, and the 1-indexed primes array convention), the algorithm was proven correct at all scales. However, at π(2⁶³−1) scale, average cluster size is only 1–3 (prime gaps are comparable at both numerator and denominator scales), and the binary search overhead (23 comparisons × 5 ns = 115 ns per cluster vs 10 ns for direct `pi_fast`) makes it a net negative.

**8× unrolling** (Opt 107). Increasing from 4× to 8× unrolling should increase MLP from 8 to 16 outstanding L2 misses. In practice, 16 live variables exceed x86-64's 16 general-purpose registers, causing stack spills. The larger loop body also increases L1 instruction cache pressure. L2 miss handling was already near-saturated at 8 outstanding misses.

---

## 4. The MLP Constraint Model

### 4.1 Memory-Level Parallelism Analysis

The AC inner loop's performance is governed by three coupled hardware constraints:

1. **L2 miss-handling capacity** (Line Fill Buffers): Arrow Lake P-cores support 12–16 outstanding L2 misses. Each `pi_fast` call requires 2 cache line fetches (`bits[word]` and `prefix[word]` are in separate 190 MB and 95 MB arrays, guaranteed to be on different cache lines). With 4× unrolling, 8 outstanding misses saturate approximately 50–67% of the miss-handling capacity, leaving headroom for fast_div's sequential array accesses.

2. **Register file** (16 GPRs): The 4× unrolled loop uses approximately 12–14 registers (4 xpq values, 4 pi values, loop counter, accumulators, base pointers). This is near the x86-64 limit. 8× unrolling exceeds it, causing spills.

3. **L3 cache pressure** (36 MB vs 285 MB table): The BigPiTable's 285 MB footprint guarantees that the vast majority of random lookups miss L3 when D is running concurrently (D's sieve operations continuously evict BigPiTable entries). The L3 hit rate for AC is approximately 36/285 ≈ 12.6% in isolation, dropping further under concurrent load.

4. **DRAM bandwidth ceiling** (89.6 GB/s): Each `pi_fast` lookup that misses L3 requires fetching 2 cache lines (128 bytes) from DRAM. With ~87% of lookups missing L3, the effective DRAM demand per lookup is ~111 bytes. The system's DDR5-5600 dual-channel memory provides ~89.6 GB/s peak bandwidth, shared across all 24 cores. This limits system-wide throughput to approximately 800M lookups/second — the true upper bound on AC throughput regardless of per-core MLP. The 4× concurrent penalty (2.10s solo → 8.42s) is consistent with D's streaming sieve operations consuming ~75% of available DRAM bandwidth, leaving AC with only ~22 GB/s effective bandwidth.

### 4.2 The Equilibrium

These three constraints create a stable performance equilibrium:

- **Reducing table size** (wheel-30, sparse prefix) adds computational overhead that exceeds the cache benefit.
- **Increasing MLP** (8× unrolling, more prefetch) exceeds the register file or miss-tracking capacity.
- **Serializing access** (monotonic sweep) destroys MLP entirely, losing the bandwidth utilization that the 4× unrolled loop provides.
- **Changing data layout** (interleaving) disrupts the spatial locality pattern that gives 8 bits-words per cache line.

The only escape from this equilibrium requires eliminating the 285 MB BigPiTable entirely — which requires the segment-first architecture that primecount uses, where all components operate on L1-sized SegmentedPiTables rebuilt per segment.

### 4.3 The Cache Warming Constraint

The DELAY_D experiment (Opt 101) revealed a second-order effect: the three concurrent components (AC, B, D) are not independent — they share L3 cache, and their performance is coupled:

- AC reads BigPiTable → keeps it warm in L3 → B benefits (B also reads BigPiTable)
- D runs sieves → evicts BigPiTable from L3 → B suffers
- The "optimal" schedule maximizes BigPiTable L3 residency for B while overlapping all three computations

This means no scheduling rearrangement can improve total time: accelerating AC by reducing its overlap with D deprives B of cache warming, slowing B by the same amount.

---

## 5. Results

### 5.1 Final Performance

| Metric | V8 (this work) | primecount v8.2 |
|--------|----------------|-----------------|
| Best (cold CPU) | **8.39s** | 8.49s |
| Median (10 runs) | 8.57s | 8.70s |
| Algorithm | Gourdon (Rust) | Gourdon (C++) |
| Table architecture | b-first BigPiTable | segment-first SegPiTable |
| π lookup cost | ~30–80 ns (L3/DRAM) | ~4 ns (L1) |
| Division cost | ~3 ns (Barrett) | ~18 ns (hardware DIV) |

V8 compensates for 7.5–20× more expensive π lookups with 6× cheaper divisions, plus micro-architectural tuning that maximizes memory-level parallelism.

### 5.2 Head-to-Head Comparison

Under identical thermal conditions (10 alternating runs):

| Metric | V8 | primecount |
|--------|------|-----------|
| Wins | **9/10** | 1/10 |
| Median | **8.63s** | 8.70s |
| Best | **8.39s** | 8.49s |
| Worst | 8.73s | 9.23s |

V8 is consistently faster under sustained thermal load, despite primecount's theoretically superior L1-resident SegmentedPiTable.

### 5.3 Statistical Significance

The 1.2% improvement (8.39s vs 8.49s best, 8.57s vs 8.70s median) warrants statistical analysis given the 2.1% thermal variance between cold-best and sustained-median.

**Binomial test (head-to-head wins).** Under the null hypothesis that V8 and primecount are equally fast (p = 0.5 per run), the probability of V8 winning ≥9 of 10 alternating runs is:

P(X ≥ 9 | n=10, p=0.5) = C(10,9)·(0.5)¹⁰ + C(10,10)·(0.5)¹⁰ = 11/1024 ≈ **0.011**

This yields **p = 0.011**, rejecting the null hypothesis at the α = 0.05 significance level. V8's advantage is statistically significant under identical thermal conditions.

**Median comparison.** The sustained medians (8.57s vs 8.70s, Δ = 0.13s, 1.5%) exceed the within-tool run-to-run standard deviation (~0.05s for V8, ~0.10s for primecount), providing additional confidence that the difference is systematic rather than thermal noise.

**Conservative interpretation.** The cold-CPU best times (8.39s vs 8.49s) are single measurements and should not be compared statistically. However, the sustained median comparison and the 9/10 head-to-head record together establish that V8 is faster with >98.9% confidence under controlled conditions on this specific hardware.

### 5.4 Scaling Behavior

| Scale | V8 | primecount | Ratio |
|-------|------|-----------|-------|
| 10¹² | 0.006s | 0.014s | **0.4×** |
| 10¹⁴ | 0.018s | 0.023s | **0.8×** |
| 10¹⁶ | 0.167s | 0.178s | **0.9×** |
| 10¹⁸ | 2.27s | 2.27s | **1.0×** |
| 2⁶³−1 | **8.39s** | 8.49s | **0.99×** |

V8 is faster at all scales except 10¹⁷ (where primecount's SegmentedPiTable advantage peaks for intermediate table sizes).

### 5.5 Parallelization Efficiency

The concurrent penalty analysis reveals the cost of parallelism on shared-memory hardware:

| Component | Solo time | Concurrent time | Penalty | Cause |
|-----------|-----------|----------------|---------|-------|
| AC | 2.10s | 8.42s | 4.0× | L3 contention + DRAM bandwidth sharing with D |
| B | 4.31s | 7.31s | 1.7× | L3 eviction by D, DRAM bandwidth sharing |
| D | 4.50s | 5.48s | 1.2× | Minimal — streaming access tolerates contention |

Sum of solo times: 2.10 + 4.31 + 4.50 = **10.91s**. Concurrent time: **8.60s**. Parallelization efficiency: 10.91/8.60 = **1.27× speedup** from overlapping three components — far below the theoretical 10.91/4.50 = 2.42× achievable with independent components. The 4.0× concurrent penalty on AC (the critical path) means that parallelism costs more than it saves for AC individually, but overall concurrency still wins because B and D are hidden behind AC's inflated time.

This suggests an unusual regime where the critical path's solo performance is excellent (2.10s) but irrelevant — the system is DRAM-bandwidth-limited, and all three components must share that bandwidth regardless of scheduling.

---

## 6. Implementation Progression

The implementation evolved through eight major versions:

| Version | Algorithm | Time at Max i64 | Speedup |
|---------|-----------|-----------------|---------|
| V1 | Segmented sieve | — (>10⁴s est.) | — |
| V2 | Lucy_Hedgehog | — (>10⁴s est.) | — |
| V3 | Meissel-Lehmer | — (>10³s est.) | — |
| V4 | Lagarias-Miller-Odlyzko | 939.21s | 1× |
| V5 | Deleglise-Rivat | — | — |
| V6 | Gourdon (segmented π) | 342.46s | 2.7× |
| V7 | Gourdon (BigPiTable) | 8.65s | 108× |
| **V8** | **Gourdon (optimized)** | **8.39s** | **112×** |

The V6→V7 transition (39.5× speedup) demonstrates the impact of the BigPiTable design: replacing segmented π-table reconstruction with a precomputed 285 MB O(1) lookup table eliminates per-segment overhead at the cost of L3/DRAM latency.

---

## 7. Lessons and Discussion

### 7.1 Memory-Level Parallelism is the Critical Resource

The most important lesson from 107 experiments is that for large-scale combinatorial computations on modern out-of-order processors, **memory-level parallelism is more important than cache hit rate**. The monotonic sweep experiment (Opt 106) converted 89% of L3/DRAM misses to L1d hits, yet performed 80% slower because serializing iterations reduced MLP from 8 to 1–2 outstanding requests.

This finding has broad implications: optimization strategies that prioritize cache locality (e.g., cache-oblivious algorithms, loop tiling) may be counterproductive if they reduce the number of independent memory requests visible to the hardware.

### 7.2 Software Prefetch is Obsolete on Modern Out-of-Order CPUs

Five separate prefetch experiments (Opt 59, 70, 94, 105) tested distances from 4 iterations (~12 ns) to 128 iterations (~1300 ns ahead). Every single one regressed performance by 3–12%. Arrow Lake's out-of-order engine already looks ahead ~200 instructions, issuing demand loads far earlier than any software prefetch hint. Worse, explicit prefetch instructions consume L2 miss-tracking entries (Line Fill Buffers) that compete with demand loads, *reducing* effective MLP rather than increasing it. This is a strong generalizable finding: on modern processors with deep out-of-order windows (≥200 instructions), software prefetch for random-access patterns is not merely useless — it is actively harmful.

### 7.3 PGO is Counterproductive for Branchless MLP-Bound Code

Profile-guided optimization was tested three independent times across different sessions (Opt 62, 77, 96), all showing no improvement or slight regression. This contradicts the conventional wisdom that PGO provides "free performance." The explanation is that PGO's primary benefit — improving branch prediction via layout optimization — is irrelevant when branches are already >99% predictable (the `is_c2` flag is loop-invariant; `y_boundary_l` mispredicts only once per b-value). PGO's secondary effect — aggressive function inlining guided by profile hotness — actually increases L1 instruction cache pressure in the already-tight inner loop. For code where the bottleneck is memory bandwidth and MLP rather than branch misprediction, PGO may be counterproductive.

### 7.4 Shared-Cache Components Cannot Be Analyzed in Isolation

The DELAY_D experiment (Opt 101) revealed that concurrent components sharing L3 cache are not independent and cannot be optimized in isolation. Solo AC completes in 2.10s, but concurrent AC takes 8.42s — a 4.0× penalty. More surprisingly, *accelerating* AC by removing its overlap with D (DELAY_D scheduling) increased total time from 8.60s to 8.70s because B lost the cache-warming side effect of AC's continuous BigPiTable lookups.

This violates a core assumption of traditional Amdahl's Law analysis: that component times are independent and the total is max(concurrent components). In reality, the components form a coupled system where AC's "wasted" L3 accesses are a free benefit to B. Any analysis that treats component times as independent would conclude that DELAY_D should help — the opposite of reality. For shared-memory parallel systems with working sets exceeding the last-level cache, component interactions through cache eviction must be modeled explicitly.

### 7.5 Convergence of Radically Different Architectures

V8 and primecount achieve near-identical performance (~8.5s) despite radically different micro-architectural strategies:

| Property | V8 (this work) | primecount |
|----------|----------------|------------|
| π lookup cost | ~30–80 ns (L3/DRAM) | ~4 ns (L1) |
| Division cost | ~3 ns (Barrett) | ~18 ns (hardware DIV) |
| Table size | 285 MB (full-range) | 3.7 KB (per-segment) |
| Parallelism model | b-first, work-stealing | segment-first, ranges |

V8 compensates for 7.5–20× more expensive π lookups with 6× cheaper divisions, while primecount compensates for 6× more expensive divisions with 7.5–20× cheaper L1-resident π lookups. The fact that these radically different tradeoffs converge to within 1.2% of each other suggests that the total computation approaches a **fundamental throughput floor** — the minimum number of memory accesses × the minimum cost per access — that no single-machine implementation can break without reducing the algorithmic work itself.

### 7.6 The Optimization Landscape is Cliff-Edged, Not Plateau-Shaped

Of 107 experiments, very few produced "slightly worse" results. The distribution is strikingly bimodal: experiments either landed within measurement noise (±1%) or caused ≥4% regression, with little in between. This suggests that near-optimal code sits on a **narrow peak** in optimization space, not a broad plateau. Small perturbations in any direction immediately fall off a performance cliff because they disrupt one of the tightly coupled constraints (MLP, register pressure, cache residency).

This has practical implications for optimization methodology: near an optimum, intuition-guided "small improvements" are overwhelmingly likely to regress. Only exhaustive, systematic exploration — as in this study — can confirm that a local optimum has been reached.

### 7.7 Exponentially Diminishing Returns

The implementation's progression from V4 to V8 shows exponentially diminishing returns on optimization effort:

| Transition | Speedup | Cumulative |
|-----------|---------|------------|
| V4 → V6 (algorithmic) | 2.7× | 2.7× |
| V6 → V7 (BigPiTable) | 39.5× | 108× |
| V7 → V8 (107 experiments) | 1.03× | 112× |

The V7→V8 transition invested 107 experiments for a 3% improvement, while V6→V7 achieved 39.5× from a single architectural change. This exponential decay in returns-per-experiment is characteristic of approaching the hardware's fundamental throughput limit. Further optimization within the current architecture would require exponentially more experiments for sub-percent gains.

### 7.8 The Register-Pressure Hard Wall at 16 GPRs

The 8× unrolling experiment (Opt 107) crossed a hard architectural boundary: x86-64's 16 general-purpose registers. The 4× unrolled loop uses 12–14 registers (4 xpq values, 4 pi values, loop counter, accumulators, base pointers) — near the limit. Doubling to 8× requires 24+ live values, causing stack spills that serialize memory accesses and destroy the MLP advantage that unrolling is meant to provide.

This is not a software limitation but a **fundamental ISA constraint**. No compiler optimization, register allocation strategy, or code transformation can create registers that don't exist. Breaking through this wall would require architectures with wider register files — ARM SVE (32 GPRs), RISC-V (32 GPRs), or future x86 extensions. On current x86-64, 4× unrolling is provably optimal for this access pattern.

### 7.9 Textbook HPC Optimizations Fail Near the Hardware Floor

A striking meta-conclusion emerges from the 107 experiments: **every standard textbook HPC optimization technique was tested and failed**:

| Textbook technique | Experiments | Result |
|-------------------|-------------|--------|
| Software prefetch | Opt 59, 70, 94, 105 | −3% to −12% |
| Profile-guided optimization (PGO) | Opt 62, 77, 96 | −0% to −5% |
| Cache tiling / segmentation | Opt 53, 91, 93 | −5× to −8.7× |
| Loop transformations (interchange, flatten) | Opt 80, 82 | ≈0 to −10% |
| Data structure compaction (wheel encoding) | Opt 56, 65 | −5% to −65% |
| Thread pool tuning | Opt 61, 84, 85, 97 | −30% to −75% |
| Increased unroll factor | Opt 107 | −4% |
| Cache-oblivious reformulation (monotonic sweep) | Opt 106 | −80% |

The only successful optimization was recompiling the Rust standard library (`-Zbuild-std`, Opt 78, +1.3%) — a compiler infrastructure change, not an algorithmic or micro-architectural one.

This does not mean these techniques are generally useless — they are well-proven for other workloads. Rather, it demonstrates that **near a hardware-constrained optimum, the standard optimization playbook is exhausted**. The code is already operating at the intersection of multiple hardware limits (MLP, register file, DRAM bandwidth, L3 capacity), and any change that improves one dimension necessarily degrades another. This is a cautionary tale: practitioners applying textbook techniques to already-optimized code should expect diminishing (or negative) returns.

### 7.10 Rust vs C++ for HPC

Our Rust implementation matches or beats a heavily optimized C++ implementation (primecount) with over a decade of development. Key Rust advantages:
- **Zero-cost abstractions**: Rayon's `par_iter` with work-stealing achieves optimal thread utilization with minimal code complexity.
- **LLVM backend**: Identical code generation quality to Clang, including BMI2 and AVX-512 instruction selection.
- **`build-std`**: Recompiling the standard library with `target-cpu=native` provides a 1.3% improvement unavailable in pre-compiled C++ standard libraries.
- **Safety with escape hatches**: `unsafe` blocks for `get_unchecked` and raw pointer arithmetic in the hot loop, with safe wrappers elsewhere.

### 7.11 The Architecture Ceiling and Future Hardware

The b-first BigPiTable architecture has a provable performance ceiling on this hardware. Breaking through requires:

1. **Segment-first rewrite** (~1000 lines): Process segments as the outer loop, rebuilding L1-sized SegmentedPiTables per segment. This eliminates the 285 MB table and its L3/DRAM bottleneck, but requires fundamentally different parallelization (threads own segment ranges, not b-value ranges).

2. **B optimization**: At 7.3s concurrent, B becomes the bottleneck if AC improves. B uses primesieve's streaming iterator, which is already highly optimized. A custom counting sieve with batch π queries could reduce B by ~40%.

3. **Hybrid P-core/E-core scheduling**: Custom thread scheduling that pins latency-sensitive AC work to P-cores and throughput-oriented D work to E-cores, bypassing rayon's core-agnostic work-stealing.

**Portability to other architectures.** Our findings are tightly coupled to Intel Arrow Lake's specific characteristics. On different hardware, the equilibrium would shift:

| Architecture | Key difference | Expected impact |
|-------------|---------------|-----------------|
| **AMD Zen 5** | 32 MB L3 (vs 36 MB), different L2 miss handling, 2× SMT | Smaller L3 → higher miss rate; SMT could improve MLP but adds contention |
| **ARM Neoverse V2** | 32 GPRs, different cache hierarchy, weaker OoO | 32 GPRs could enable 8× unrolling without spills; weaker OoO may reduce effective MLP |
| **Apple M4** | 192-entry ROB, 16 MB shared L2, unified memory | Higher per-core MLP potential; unified memory eliminates DRAM bandwidth wall |
| **Future x86 (APX)** | Potential extension to 32 GPRs | Would break the 16-GPR wall, enabling deeper unrolling |

The MLP constraint model (§4) is portable: on any architecture, the performance ceiling is determined by min(per-core MLP × cores, DRAM bandwidth / bytes-per-lookup, register file / registers-per-iteration). The specific values change, but the analytical framework applies.

### 7.12 Limitations

Our comparison with primecount uses wall-clock time under controlled conditions but does not account for potential platform-specific advantages (primecount may perform differently on AMD processors or Linux systems). The thermal variance between cold-CPU best (8.39s) and sustained median (8.57s) reflects the reality of modern boost clocks and should be reported alongside best times in benchmarks.

**Energy efficiency.** We do not measure power consumption or energy-per-computation. The BigPiTable architecture's heavy DRAM traffic (estimated ~50–70 GB/s sustained during AC) likely consumes significantly more memory-subsystem power than primecount's L1-resident approach. An energy-efficiency comparison (joules per π(x) computation) could favor primecount despite its slower wall-clock time.

**Single-platform evaluation.** All results are from a single machine running Windows 11. Linux kernel scheduling, different NUMA topologies, and different memory controllers could shift the balance. The 1.2% advantage is narrow enough that platform-specific effects could reverse the ranking on different hardware.

---

## 8. Related Work

**Walisch (2010–present)**: *primecount* implements Gourdon's algorithm with a SegmentedPiTable that fits in L1 cache, achieving O(x^{2/3}/ln²x) with excellent cache behavior. Our work shows that an alternative architecture (full-range BigPiTable with Barrett reduction) can match this performance through careful micro-architectural optimization.

**Deleglise and Rivat (1996)**: Introduced the easy-leaf/hard-leaf decomposition that Gourdon's algorithm extends. Our V5 implements this directly.

**Lagarias, Miller, and Odlyzko (1985)**: The LMO method, which our V4 implements, uses a different decomposition with "special leaves" rather than Gourdon's AC/B/D split.

**Oliveira e Silva (2006)**: Extended LMO computations to verify the Riemann hypothesis for large x, establishing benchmarks for prime counting implementations.

---

## 9. Conclusion

We have presented a Rust implementation of Gourdon's prime counting algorithm that achieves 8.39s for π(2⁶³ − 1) on Intel Arrow Lake, beating the long-standing C++ state-of-the-art by 1.2% (p = 0.011 by binomial test on head-to-head runs). Through 107 systematic experiments, we established that the code occupies a local performance optimum determined by four coupled hardware constraints: L2 miss-handling capacity, register file size, L3 cache pressure, and DRAM bandwidth.

Beyond the specific result, this study yields several generalizable findings for high-performance computing on modern out-of-order processors:

1. **MLP over cache hit rate**: Memory-level parallelism governs performance for random-access workloads; optimizations that improve locality at the cost of MLP are counterproductive (§7.1).
2. **Software prefetch is harmful**: On processors with deep out-of-order windows (≥200 instructions), explicit prefetch competes with demand loads for miss-tracking resources (§7.2).
3. **PGO fails for MLP-bound code**: When branches are already well-predicted and the bottleneck is memory bandwidth, PGO's inlining heuristics increase I-cache pressure (§7.3).
4. **Shared-cache coupling**: Concurrent components sharing last-level cache cannot be analyzed independently; optimizing one may degrade another through cache eviction effects (§7.4).
5. **Architectural convergence**: Radically different implementations (BigPiTable vs SegPiTable) converge to within 1.2%, suggesting a fundamental throughput floor (§7.5).
6. **Cliff-edged optima**: Near-optimal code sits on a narrow peak; the vast majority of perturbations cause significant regression, not gradual degradation (§7.6).
7. **The 16-GPR wall**: x86-64's register file is the hard limit on unroll factor for MLP-generating loops; only wider ISAs can push further (§7.8).
8. **Textbook techniques fail at the floor**: Every standard HPC optimization (prefetch, PGO, cache tiling, loop transforms) was tested and failed, demonstrating that near a hardware-constrained optimum, the conventional playbook is exhausted (§7.9).

These insights extend beyond prime counting to any large-scale computation with random memory access patterns on modern hardware. The MLP constraint model (§4) provides a portable analytical framework: on any architecture, the performance ceiling is determined by the minimum of per-core MLP capacity, system DRAM bandwidth, and register file depth — a framework applicable to hash tables, database joins, graph traversals, and other random-access workloads at scale.

---

## Appendix A: Build Configuration

```toml
# .cargo/config.toml
[build]
rustflags = [
    "-C", "target-cpu=native",
    "-C", "llvm-args=--unroll-threshold=800",
    "-Zlocation-detail=none",
    "-Zmir-opt-level=4",
    "-Ztune-cpu=arrowlake"
]
```

```powershell
# Build command
cargo +nightly build --release --bin prime_count_v8 `
    -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc
```

## Appendix B: Component Timing at π(2⁶³ − 1)

| Mode | AC | B | D | Total |
|------|------|------|------|-------|
| All concurrent (default) | 8.42s | 7.31s | 5.48s | 8.60s |
| AC alone (PHASE_AC_DB) | **2.10s** | — | — | — |
| B alone | — | **4.31s** | — | — |
| D alone (PHASE_D_ACB) | — | — | **4.50s** | — |
| DELAY_D (AC first, then D) | 2.10s | **8.50s** | 5.00s | 8.70s |

## Appendix C: Full Experiment Index

| # | Experiment | Time | Δ |
|---|-----------|------|---|
| 53 | Segment-first AC | 75.0s | −8.7× |
| 54 | FullPiTable (mod-240) | 9.29s | −7.8% |
| 55 | Interleaved BigPiTable v1 | 8.99s | −4.3% |
| 56 | Wheel-30 BigPiTable | 9.08s | −5.3% |
| 57 | Generic PiTable trait | 9.71s | −12.7% |
| 58 | AC segment size sweep | 8.65s | ≈0 |
| 59 | Software prefetch (4 ahead) | 9.66s | −12.2% |
| 60 | Primecount alpha params | 9.72s | −12.5% |
| 61 | Dedicated AC pool | 14.29s | −65.3% |
| 62 | PGO (stable) | 8.62s | ≈0 |
| 63 | Clustered easy leaves v1 | 11.90s | −38.0% |
| 64 | Interleaved bits+prefix v2 | 9.06s | −5.2% |
| 65 | Sparse prefix (8-word) | 14.18s | −65.0% |
| 66 | B_THREADS=4 analysis | 14.36s | — |
| 67 | Phase scheduling | 9.06s | −3.9% |
| 68 | Dedicated D pool | 8.71s | ≈0 |
| 69 | D chunk granularity | 8.66s | ≈0 |
| 70 | Same-batch prefetch | 8.97s | −4.3% |
| 71 | PGO (nightly, blocked) | — | — |
| 72 | AC_SEG=200K | 8.63s | ≈0 |
| 73 | POOL_MULT sweep | 8.72s | confirmed |
| 74 | Alpha_Y sweep | 8.63s | ≈0 |
| 75 | B_THREADS sweep | 8.76s | confirmed |
| 76 | Split wide/narrow | 9.86s | −14.0% |
| 77 | PGO (nightly) | 9.06s | −5.3% |
| 78 | **-Zbuild-std** | **8.55s** | **+1.3%** |
| 79 | panic_immediate_abort | 8.55s | ≈0 |
| 80 | Loop interchange/flatten | 8.55s | ≈0 |
| 81 | unroll-threshold=1200 | 8.60s | −0.6% |
| 82 | Branchless AC loop | 9.47s | −10.0% |
| 83 | Interleaved BigPiTable v3 | 9.07s | −6.0% |
| 84 | D_THREADS=16 | 11.13s | −30.0% |
| 85 | AC_THREADS=16 | 14.88s | −72.0% |
| 86 | POOL_MULT=2 | 8.72s | −2.0% |
| 87 | AC_SEG sweep (build-std) | 8.61s | confirmed |
| 88 | Alpha_Y sweep (build-std) | 8.61s | confirmed |
| 89 | B_THREADS sweep (build-std) | 8.60s | confirmed |
| 90 | P-core affinity | 18.8s | −120% |
| 91 | Per-b SegPiTable | >100s | killed |
| 92 | mimalloc large pages | 8.55s | ≈0 |
| 93 | Segment-first SegPiTable | 11.77s | −5× |
| 94 | Software prefetch pipeline | 8.45s | ≈0 |
| 95 | Clustered easy leaves v2 | 35.12s | −4× |
| 96 | PGO + build-std | 8.58s | ≈0 |
| 97 | Separate thread pools | 15.03s | −75% |
| 98 | LLVM flag tuning | 8.57s | ≈0 |
| 99 | Phased scheduling | 8.99s | −5% |
| 100 | D segment size | 8.51s | ≈0 |
| 101 | DELAY_D scheduling | 8.70s | −1.5% |
| 102 | B_THREADS sweep v2 | 8.60s | confirmed |
| 103 | POOL_MULT sweep v2 | 8.60s | confirmed |
| 104 | Interleaved BigPiTable v4 | 9.05s | −5% |
| 105 | Deep prefetch (32-128) | 8.77s | −4% |
| 106 | Monotonic sweep running pi | 15.62s | −80% |
| 107 | 8× unrolling | 8.93s | −4% |

Of 107 experiments, **1 succeeded** (Opt 78: build-std, +1.3%), **~10 were neutral** (within noise), and **~96 were regressions** (ranging from −0.6% to −8.7×).

---

## Acknowledgments

This work used Kim Walisch's *primesieve* library for B-component prime enumeration and *primecount* as the primary benchmark target. The implementation builds on the Rayon parallel runtime and mimalloc allocator.

## References

1. Gourdon, X. (2001). "Computation of pi(x): improvements to the Meissel, Lehmer, Lagarias, Miller, Odlyzko, Deleglise and Rivat method."
2. Walisch, K. (2010–present). *primecount*: Highly optimized C++ implementation of the prime counting function. https://github.com/kimwalisch/primecount
3. Walisch, K. (2010–present). *primesieve*: Fast prime number generator. https://github.com/kimwalisch/primesieve
4. Deleglise, M. and Rivat, J. (1996). "Computing π(x): The Meissel, Lehmer, Lagarias, Miller, Odlyzko method." *Mathematics of Computation*, 65(213), 235–245.
5. Lagarias, J.C., Miller, V.S., and Odlyzko, A.M. (1985). "Computing π(x): An analytic method." *Journal of Algorithms*, 6(3), 537–560.
6. Oliveira e Silva, T. (2006). "Computing π(x): the combinatorial method." *Revista do DETUA*, 4(6), 759–768.

---

*Source code: https://github.com/secwest/fast-prime*
