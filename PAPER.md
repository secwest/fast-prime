# Beating Primecount: Exhaustive Micro-Architecture-Aware Optimization of Combinatorial Prime Counting

**Dragos Ruiu**

---

## Abstract

How fast can you count the primes below 2⁶³? Kim Walisch's *primecount* -- the state-of-the-art C++ implementation, refined over a decade -- does it in 8.76 seconds. We beat that by 3.65%. But this is not really a story about prime counting.

Our Rust implementation of Gourdon's algorithm computes π(2⁶³ − 1) = 216,289,611,853,439,384 in **8.38 seconds** on an Intel Core Ultra 9 285K. We arrived here through **321+ optimization experiments across eight algorithm versions**, spanning segmented sieves, combinatorial methods, LMO, Deleglise-Rivat, and Gourdon's algorithm. The early experiments succeeded at high rates: 79% in V4 (LMO), 83% in V5-V6 (Deleglise-Rivat), 87% in V7 (Gourdon). Then, in V8, we hit the wall. Over 167+ additional experiments across 14 sessions, the success rate collapsed to 2%. Only three infrastructure-level changes helped: recompiling the Rust standard library with target-specific codegen (+1.3%), enabling 2MB large pages via mimalloc (+1.3%), and a compact two-level pi table that reduced memory contention (-1.2% wall time). Every algorithmic or micro-architectural change -- software prefetch, profile-guided optimization, cache tiling, loop restructuring, data structure compaction, thread pool isolation, streaming per-segment sieves -- made things worse, often dramatically.

This is a story about approaching and then reaching what appears to be the hardware floor, and what you find when you get there: a stable equilibrium where four hardware constraints (L2 miss-handling capacity, register file size, DRAM bandwidth, and L3 cache pressure) interlock so tightly that every improvement attempted on one dimension degrades another. The collapse in optimization success rate -- from 80-90% to 2% -- is itself a measurable signature of the floor. The final proof came from implementing and benchmarking a streaming AC approach that eliminated the 285MB lookup table entirely, replacing it with L2-cached per-segment sieves, only to discover that Arrow Lake's memory-level parallelism (38 outstanding DRAM loads from a 512-entry reorder buffer) already achieves effective per-load latency comparable to L2 cache, making the optimization pointless. It is also a story about how a human and an AI worked together to get there -- a researcher directing strategy while an LLM implemented code changes, executed experiments, diagnosed 20 correctness bugs, and maintained documentation across sessions interrupted by power failures and context limits. The human brought the intuition ("try Barrett reduction"); the AI brought the stamina (321+ experiments, each requiring build-benchmark-analyze-revert cycles). Neither could have done this alone in the same timeframe.

Along the way, we discovered that improving cache hit rate can *slow things down* by 80%, that making one component faster can paradoxically slow the *total* computation, and that every textbook HPC optimization technique fails near the hardware floor. We catalogued 20 correctness bugs and found that 45% were off-by-one errors -- the eternal nemesis of number-theoretic code, from Meissel's manual miscalculation of π(10⁹) in 1885 to our SegmentedPiTable indexing error in 2025. These findings -- detailed across 25 lessons in §7 -- apply broadly to any memory-bound computation on modern out-of-order processors.

---

## 1. Introduction

### 1.1 The Prime Counting Function

The prime counting function π(x) -- the number of primes not exceeding x -- is among the most fundamental functions in analytic number theory. The question "how many primes are there below N?" has been asked since antiquity, but efficient computation at scale has driven over 150 years of algorithmic innovation.

**The prime number theorem.** Gauss conjectured in 1792 (at age 15) that π(x) ≈ x / ln x. Hadamard and de la Vallée-Poussin independently proved this in 1896, establishing that π(x) ~ Li(x), where Li(x) = ∫₂ˣ dt/ln t. But the prime number theorem gives only an approximation -- computing π(x) *exactly* requires entirely different methods.

**Legendre's combinatorial formula (1808).** The first systematic approach: express π(x) in terms of an inclusion-exclusion sieve. Legendre's formula computes π(x) in O(x / ln ln x) time -- impractical for large x, but the starting point for all subsequent combinatorial methods.

**Meissel's breakthrough (1870–1885).** Ernst Meissel introduced a clever recursive decomposition that reduces the computation to O(x^{2/3} / ln x) time, replacing the full sieve with a combination of smaller subproblems. He computed π(10⁸) = 5,761,455 and π(10⁹) = 50,847,478 **by hand** -- a feat of extraordinary patience. His value for π(10⁹) was later found to be wrong by 56 (the correct value is 50,847,534), an error not discovered for over 70 years.

**Lehmer's electronic computation (1959).** Derrick Henry Lehmer adapted Meissel's method for the IBM 701, computing π(10¹⁰) = 455,052,512. This too was wrong -- by exactly 1 (the correct value is 455,052,511). Lehmer's algorithm formalized the partial sieve function φ(x, a) that remains central to all modern methods. His error, like Meissel's, illustrates the treacherous arithmetic of combinatorial prime counting: a single off-by-one in a sub-formula propagates silently through billions of operations.

**Lagarias, Miller, and Odlyzko (1985).** The LMO algorithm achieved a major complexity improvement to O(x^{2/3+ε}) time and O(x^{1/3+ε}) space by restructuring the sieve computation into "ordinary" and "special" leaves processed over segmented intervals. They computed π(4 × 10¹⁶). This decomposition enabled the first practical computations beyond 10¹⁵.

**The analytic method (Lagarias & Odlyzko, 1987).** An entirely different approach: compute π(x) via a contour integral involving the Riemann zeta function ζ(s). Achieving O(x^{1/2+ε}) time -- asymptotically faster than any combinatorial method -- it requires computing billions of zeta zeros. The enormous constant factors make it impractical below ~10²⁰, but David Platt (2012) used it to verify π(10²⁴) using ~70 billion zeta zeros with interval arithmetic.

**Deléglise and Rivat (1996).** Built on LMO with the critical innovation of decomposing the computation into "easy leaves" and "hard leaves," achieving O(x^{2/3} / ln x) time with practical constant factors. Their implementation computed π(10¹⁸) -- the first time this threshold was crossed.

**Gourdon (2001).** Xavier Gourdon refined the Deléglise-Rivat decomposition into the five-component formula used by all modern implementations:

$$\pi(x) = \text{AC}(x) - B(x) + D(x) + \Phi_0(x) + \Sigma(x)$$

achieving O(x^{2/3} / ln² x) time and O(x^{1/3} ln³ x) space. The AC term (easy leaves) dominates computation time; B computes a P₂-equivalent sum; D handles hard leaves via segmented sieve; Φ₀ and Σ are small correction terms. Gourdon's decomposition enabled the computation of π(10²³) by Oliveira e Silva in 2007.

**Modern records.** Kim Walisch's *primecount* -- the current state-of-the-art open-source implementation -- uses Gourdon's algorithm with a SegmentedPiTable architecture optimized for L1 cache. Walisch and David Baugh have computed:

| Year | Record | Value |
|------|--------|-------|
| 2015 | π(10²⁷) | 16,352,460,426,841,680,446,427,399 |
| 2020 | π(10²⁸) | 157,589,269,275,973,410,412,739,598 |
| 2022 | π(10²⁹) | 1,520,698,109,714,272,166,094,258,063 |

Each computation was run twice with independent parameters to guard against hardware errors -- a testament to the combinatorial fragility that has produced wrong results since Meissel.

**Our work** implements Gourdon's formula with a fundamentally different data structure architecture (BigPiTable + Barrett reduction vs SegmentedPiTable + hardware division), arriving at competitive performance via a different point in the tradeoff space. We are, to our knowledge, the first Rust implementation to match or beat primecount at the 2⁶³ scale.

### 1.2 Motivation and Contributions

This work began as a question: *can a human and an AI, working together, match a decade of expert optimization in a few weeks?* The human (a security researcher with systems programming experience but no prior work in analytic number theory) brought domain intuition, architectural decisions, and the willingness to chase diminishing returns. The AI (GitHub Copilot CLI, powered by Claude) brought implementation speed -- writing, compiling, benchmarking, profiling, reverting, and documenting code changes in rapid cycles that would be unsustainable for a human working alone. The result was 321+ optimization experiments across 8 algorithm versions in approximately 120 hours of interactive sessions -- an experiment every 22 minutes, sustained over weeks.

What we found was unexpected. The exercise in "can we make this faster" became a systematic study of *what happens when you try to optimize code that is already near the hardware floor*. Over eight implementation versions and 321+ experiments, we pushed performance from over 10,000 seconds (V1 segmented sieve) to 8.38 seconds -- a greater than 1,000x improvement -- with each version contributing successful optimizations until V8, where the success rate collapsed from 80-90% to 2%. The story of that collapse, and the hardware constraints that cause it, is the core contribution of this paper.

The collaboration itself produced lessons. The AI excelled at the mechanical cycle -- edit, build, benchmark, analyze, revert -- and at maintaining perfect documentation of every experiment (essential for later analysis). The human excelled at the strategic pivots: "Barrett reduction might be the answer," "we need to try a completely different algorithm," "that failure pattern means the bottleneck is elsewhere." Neither skillset alone could have produced this work. A human working solo would have run perhaps 40-60 experiments before fatigue; an AI working unsupervised would have explored locally without the strategic leaps (V3->V4, V6->V7) that provided 99% of the improvement.

Specifically, our contributions are:

1. **A competitive Rust implementation** that matches or beats primecount at all scales from 10^10 to 2^63 - 1, demonstrating that Rust's zero-cost abstractions and LLVM backend can achieve C++-competitive performance for memory-bound numerical workloads.

2. **A taxonomy of 321+ optimization experiments across 8 versions** with detailed performance analysis, providing an empirical map of the optimization landscape -- from high-success-rate algorithmic improvements through to the final 2% success rate at the hardware floor.

3. **Identification of the MLP constraint** as the apparent performance limiter: the 4x unrolled inner loop generates 8 independent L2 miss requests that saturate approximately 38 of the processor's ~48 L2 MSHRs, creating an equilibrium that resisted improvement by every local code transformation we attempted. Definitive proof via streaming AC experiment.

4. **Discovery of the BigPiTable L3 warming effect**: the AC computation's continuous random accesses to the 285 MB π-table keep it warm in L3 cache, benefiting the concurrent B computation. Scheduling changes that accelerate AC paradoxically slow the overall computation by depriving B of this cache warming.

5. **Quantification of the concurrent penalty**: AC alone completes in 2.10s, but concurrent execution with D inflates this to 8.42s -- a 4.0× penalty from L3 cache pressure, work-stealing overhead, and power throttling. We found no way to reduce this penalty within the current architecture.

6. **A case study in human-AI collaborative research**: demonstrating that an LLM-assisted workflow can compress months of optimization work into weeks, while producing documentation (this paper, 1100+ lines of optimization logs, 2700+ lines of thought log) that would typically be omitted from a solo effort.

### 1.3 Hardware Platform

All experiments were conducted on:

| Component | Specification |
|-----------|--------------|
| CPU | Intel Core Ultra 9 285K (Arrow Lake) |
| P-cores | 8 × Lion Cove, 5.7 GHz, 2 MB L2 each |
| E-cores | 16 × Skymont, 4.6 GHz, 4 MB L2 per 4-core cluster |
| L3 cache | 36 MB shared |
| Memory | 96 GB DDR5-6600 dual channel (~105.6 GB/s) |
| Hyperthreading | None (24 total hardware threads) |
| OS | Windows 11 |
| Compiler | Rust 1.95.0-nightly, LLVM 19 |

### 1.4 Paper Organization and Key Surprises

**§2** describes the algorithm and implementation, including the two key data structures (BigPiTable for O(1) π lookups, Barrett reduction for fast division) and our experimental methodology. **§3** catalogs the 321+ experiments, presenting the V8 experiments in detail across seven optimization axes, including the definitive streaming AC experiment that proved the hardware ceiling. **§4** presents the MLP constraint model -- our best explanation for why the code resisted improvement, refined by the discovery of MSHR-saturated memory-level parallelism. **§5** gives performance results with statistical analysis. **§6** traces the implementation's evolution through 8 major versions and 321+ experiments -- the narrative heart of the paper, showing how optimization success rates collapsed from 80-90% to 2% as the code approached the hardware floor. **§7** draws 25 lessons from the experiments. **§8** places our work in the context of 150 years of prime counting algorithms. **§9** concludes with findings about hardware, software, correctness, and the human-AI collaboration that produced this work.

For the reader short on time, the most surprising and broadly applicable findings are:

- **Improving cache hit rate made things 80% slower** (§7.1) -- converting L3 misses to L1 hits destroyed memory-level parallelism, proving the loop is bandwidth-bound, not latency-bound.
- **Making one component faster slowed the whole system** (§7.4) -- accelerating AC deprived B of a cache-warming side effect, a violation of the independence assumption in Amdahl's Law.
- **Every textbook optimization failed** (§7.9) -- prefetch, PGO, cache tiling, loop transforms, data compaction: all tested, all regressed.
- **DRAM can beat L1** (§7.14) -- a 285 MB table with high MLP outperforms 23,000 rebuilds of a 3.7 KB L1-resident table, yielding a 39.5× speedup.
- **Optimizations destroy each other** (§7.10) -- near the hardware floor, every improvement we tried on one dimension degraded another.
- **Off-by-one errors are the dominant bug category** (§7.16) -- 45% of 20 bugs, invisible at small scales, catastrophic at large scales. Meissel got π(10⁹) wrong in 1885 for the same reason we got SegmentedPiTable wrong in 2026.

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

This compiles to BMI2 `BZHI` + `POPCNT` on the critical path -- near the hardware minimum for a table-based π lookup.

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

The AC computation is the critical path (8.23s concurrent, 2.1s solo). Its inner loop processes 154,701 b-values, each iterating over a range of l-values:

```
for each b-value (parallel over rayon global pool):
    for l in eff_lo..=eff_hi (4× unrolled):
        xpq = fast_div(x/p_b, p_l, recip_l)
        pi_val = big_pi.pi_fast(xpq)
        accumulate(pi_val, ...)
```

The 4× unrolling generates 4 independent `fast_div` + `pi_fast` chains per loop body, creating 8 independent memory requests (2 per `pi_fast`: one for `bits[word]`, one for `prefix[word]`).

### 2.5 Experimental Methodology

All 167+ experiments followed a controlled benchmarking protocol:

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
4. **Regression testing during optimization**: Each of the 167+ experiments verified correctness before and after code changes. Several experiments (Opt 95 clustered leaves, Opt 106 monotonic sweep, Opt 154 streaming AC) uncovered bugs that were fixed and re-verified before benchmarking.

---

## 3. Optimization Taxonomy

The 321+ experiments across all versions tell a story of optimization in two phases: a **productive phase** (V1-V7, ~154 experiments, ~119 succeeded) where standard techniques worked well, and a **ceiling phase** (V8, 167+ experiments, 3 succeeded) where almost nothing worked. The collapse in success rate is itself a signal: it marks the transition from "code that can be improved" to "code that has reached the hardware floor."

This section presents the V8 ceiling-phase experiments in detail, organized by optimization axis, explaining not just the result but the *mechanism* of failure -- because the failures reveal the hardware constraints that define the performance ceiling. The productive-phase experiments (V1-V7) are summarized in §6.

### 3.0 The Optimization Success Rate Collapse

The most striking pattern in the 321+ experiments is the success rate by version:

| Version | Algorithm | Experiments | Succeeded | Success Rate | Key Wall |
|---------|-----------|------------|-----------|-------------|----------|
| V1 | Segmented sieve | ~34 | ~19 | 56% | O(N) scaling |
| V2 | Lucy_Hedgehog | ~11 | ~10 | 91% | O(N^{3/4}) scaling |
| V3 | Meissel-Lehmer | ~6 | ~1 | 17% | Pipeline saturation at 3.5 cyc/op |
| V4 | LMO | ~39 | ~31 | 79% | S2_hard sieve bottleneck |
| V5 | Deleglise-Rivat | ~6 | ~5 | 83% | S2_hard at 81s immovable |
| V6 | Gourdon (segmented pi) | ~6 | ~5 | 83% | Per-segment rebuild cost |
| V7 | Gourdon (BigPiTable) | ~52 | ~45 | 87% | Approaching DRAM bandwidth limit |
| **V8** | **Gourdon (optimized)** | **167+** | **3** | **2%** | **MLP-MSHR-register equilibrium** |

The 87% -> 2% collapse between V7 and V8 marks the hardware floor. V7's final optimization (Opt 52: early AC/B/C1 start) brought wall time to 8.65s. V8's 167+ experiments -- the most exhaustive optimization search in the project -- gained only 3.2% more (8.65s -> 8.38s). The wall is real.

### 3.1 Summary of V8 Experiments (167+ across 14 sessions)

We categorize the 167+ V8 experiments into seven axes:

| Axis | Experiments | Best result | Key finding |
|------|-------------|------------|-------------|
| **Data structure layout** | 55, 56, 64, 65, 83, 104, 117, 122, 125, 135 | CompactPi -1.2% | Odd-only encoding is Pareto-optimal; memory reduction helps D/B but not AC |
| **Cache/memory** | 53, 54, 59, 70, 91, 93, 94, 105, 106, 108, 129, 145, 149, 154 | Large pages +1.3% | MLP-38 makes DRAM nearly as fast as L2; streaming AC is non-viable |
| **Thread scheduling** | 61, 66-68, 84-86, 97, 99, 101-103, 124, 126, 128, 136, 140, 141 | Defaults optimal | Work-stealing > dedicated pools; all isolation attempts catastrophic |
| **Compiler/build** | 62, 71, 77-81, 96, 98, 121, 127, 130 | +1.3% (build-std) | PGO, LTO variants, process priority all counterproductive or neutral |
| **Algorithmic** | 60, 63, 82, 95, 100, 109, 110, 114, 119, 137, 142-144, 150, 156 | Alpha confirmed | Clustering needs L1-resident table; all parameter sweeps confirm optimum |
| **Loop structure** | 76, 107, 118, 151-153 | All worse | 4x unroll is Pareto-optimal; MLP invariant to unroll factor (ROB-limited) |
| **Streaming architecture** | 154-155 | 8.90s (best strategy) | MLP-38 already achieves L2-equivalent latency; segment sieves add overhead |

### 3.2 Data Structure Layout Experiments

**Interleaved bits+prefix** (Opt 55, 64, 83, 104). We repeatedly tested merging the `bits[]` and `prefix[]` arrays into a single interleaved `data[]` array, placing both values for the same word on the same cache line. Despite the theoretical halving of DRAM accesses per lookup, every attempt showed 4–6% regression. The root cause: interleaving reduces bits-per-cache-line from 8 to 4, halving spatial locality for the semi-sequential access pattern within each b-value's l-iteration.

**Wheel-30 encoding** (Opt 56). Encoding 240 numbers per u64 (8 coprime-to-30 residues) reduces the table from 285 MB to 152 MB. However, the division by 240 adds ~4 cycles per lookup versus the shift-only indexing of odd-only encoding (n >> 7). The computational overhead exceeds the cache benefit.

**Sparse prefix** (Opt 65). Replacing the per-word prefix array (95 MB) with per-8-word coarse checkpoints (12 MB) requires up to 7 additional `POPCNT` instructions per lookup. The branch-heavy conditional popcounting generates unpredictable branches, causing a 65% regression.

### 3.3 Cache and Memory Access Experiments

**Software prefetch** (Opt 59, 70, 94, 105). We tested prefetch distances from 4 iterations (~12 ns) to 128 iterations (~1300 ns). Short distances (4 iterations) are redundant -- Arrow Lake's out-of-order engine already looks ahead ~200 instructions. Long distances (32–128 iterations) consume L2 miss-tracking entries that compete with demand loads, reducing effective MLP. All prefetch experiments showed 3–12% regression.

**Segment-first SegmentedPiTable** (Opt 53, 91, 93). Primecount achieves L1-resident π lookups via a 3.7 KB per-segment sieve rebuilt per segment. We attempted three implementations:
- Opt 53: Full segment-first architecture (75s -- 8.7× regression)
- Opt 91: Per-b-value streaming sieve (killed -- redundant sieve rebuilds)
- Opt 93: Per-thread chunk with sub-segments (11.77s -- 5× regression)

The fundamental issue: sieve construction cost is O(√x · ln ln √(√x)) per thread -- a fixed overhead of ~2.78s/thread that exactly cancels the L1 cache benefit of ~0.66s savings from 220M lookups at 3 ns each. Primecount avoids this by using a segment-first outer loop (build sieve once per segment, process all b-values), which requires a fundamentally different parallelization strategy.

**Monotonic sweep** (Opt 106). Since xpq decreases monotonically within each b-value, we maintained a running π counter updated via sequential L1d-cached sieve scans. Despite ~89% of iterations having Δxpq < 4096 (suitable for scanning), the serialization of iterations destroyed 4× unrolling MLP, causing an 80% regression. This experiment definitively proved the loop is bandwidth-bound, not latency-bound.

### 3.4 Thread Scheduling and Pool Configuration

**Phase scheduling** (Opt 67, 99). Running components sequentially reveals their solo performance:
- AC alone: **2.10s** (vs 8.42s concurrent -- **4.0× penalty**)
- B alone: 4.31s (vs 7.31s concurrent -- 1.7× penalty)
- D alone: 4.50s (vs 5.48s concurrent -- 1.2× penalty)

Despite the massive concurrent penalty, phased execution is slower (8.99–9.36s) because sequential phases cannot overlap, and the overlap savings (max(8.42, 7.31, 5.48) vs 2.10 + 7.31 + 4.50) favor concurrency.

**DELAY_D scheduling** (Opt 101). Waiting for AC to finish before starting D yielded a critical discovery: B slowed from 7.31s to 8.50s. **AC's "concurrent penalty" is partially beneficial** -- its continuous BigPiTable lookups keep the 285 MB table warm in L3 cache for B's benefit. When AC finishes early, D's sieve operations evict BigPiTable, slowing B.

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

The pattern across 321+ experiments is clear: V1-V7 optimizations succeeded because they were working above the hardware floor, while V8's 167+ experiments failed because the code had reached it. This section explains *why* by identifying the four coupled hardware constraints that create an inescapable equilibrium.

### 4.1 Memory-Level Parallelism Analysis

The AC inner loop's performance is governed by four coupled hardware constraints, quantified precisely through hot-loop disassembly analysis (Opt 151) and MSHR saturation modeling (Opt 152):

1. **MSHR saturation** (Miss Status Holding Registers): Disassembly reveals the 4x unrolled loop body is ~92 micro-ops. With a 512-entry ROB, the CPU holds ~5.6 groups in flight, generating ~44.5 outstanding loads. At an ~87% L3 miss rate, approximately **38 loads are outstanding DRAM requests** at any given moment, consuming ~80% of the estimated ~48 L2 MSHRs. This near-saturation explains why adding *any* instructions (software prefetch, wheel-30 decode, interleaved layout) reduces effective MLP: additional micro-ops shrink the ROB window, reducing in-flight loads.

2. **Register file** (16 GPRs): The 4x unrolled loop uses 12-14 registers (4 xpq values, 4 pi values, loop counter, accumulators, base pointers). This is near the x86-64 limit. 8x unrolling exceeds it, causing stack spills. Analysis (Opt 153) proved that 8x gives identical MLP (512/184 = 2.78 groups x 16 loads = 44.5, same as 4x), while the code bloat causes I-cache regression.

3. **L3 cache pressure** (36 MB vs 285 MB table): The BigPiTable's 285 MB footprint guarantees that the vast majority of random lookups miss L3 when D is running concurrently (D's sieve operations continuously evict BigPiTable entries). The L3 hit rate for AC is approximately 36/285 = 12.6% in isolation, dropping further under concurrent load.

4. **DRAM bandwidth ceiling** (105.6 GB/s): Each `pi_fast` lookup that misses L3 requires fetching 2 cache lines (128 bytes) from DRAM. With ~87% of lookups missing L3, the effective DRAM demand per lookup is ~111 bytes. The system's DDR5-6600 dual-channel memory provides ~105.6 GB/s peak bandwidth, shared across all 24 cores. The ~3.9x concurrent penalty (2.1s solo -> 8.23s) is consistent with D's streaming sieve operations consuming ~75% of available DRAM bandwidth, leaving AC with only ~26 GB/s effective bandwidth.

### 4.2 The Equilibrium

These four constraints create a stable performance equilibrium:

- **Reducing table size** (wheel-30, sparse prefix) adds computational overhead that exceeds the cache benefit. Wheel-30 (Opt 122) added 36 micro-ops per group, reducing ROB-visible groups from 5.6 to 3.8 and effective MLP by 32%, despite halving the table size.
- **Increasing MLP** (8x unrolling, more prefetch) exceeds the register file or MSHR capacity. Speculative prefetch (Opt 152) would push outstanding requests from ~38 to ~57, exceeding the ~48 MSHRs.
- **Serializing access** (monotonic sweep) destroys MLP entirely, losing the bandwidth utilization that the 4x unrolled loop provides.
- **Changing data layout** (interleaving) disrupts the spatial locality pattern that gives 8 bits-words per cache line.
- **Eliminating the table** (streaming AC, Opt 154) replaces DRAM random access with L2-cached sieves, but Arrow Lake's MLP-38 already achieves effective per-load latency of ~100ns/38 = 2.6ns -- comparable to L2's ~2.5ns. The streaming approach adds sieve construction overhead (0.7s), load imbalance (segment 0 has 69% of work), and nested parallelism degradation, making it strictly worse.

There is no remaining escape from this equilibrium within the current algorithmic framework. The streaming AC experiment (Session 14) was the definitive test: even eliminating the 285 MB BigPiTable entirely cannot improve performance, because the hardware's MLP already amortizes the DRAM latency to near-L2 levels.

### 4.3 The Cache Warming Constraint

The DELAY_D experiment (Opt 101) revealed a second-order effect: the three concurrent components (AC, B, D) are not independent -- they share L3 cache, and their performance is coupled:

- AC reads BigPiTable → keeps it warm in L3 → B benefits (B also reads BigPiTable)
- D runs sieves → evicts BigPiTable from L3 → B suffers
- The "optimal" schedule maximizes BigPiTable L3 residency for B while overlapping all three computations

This means no scheduling rearrangement we tested could improve total time: accelerating AC by reducing its overlap with D deprived B of cache warming, slowing B by a comparable amount.

---

## 5. Results

Although the constraint model suggests that no local optimization should improve performance, V8 achieves a measurable advantage over primecount through its different architectural tradeoff: trading expensive π lookups for cheap Barrett divisions.

### 5.1 Final Performance

| Metric | V8 (this work) | primecount v8.2 |
|--------|----------------|-----------------|
| Best (cold CPU) | **8.38s** | 8.72s |
| Median (5 runs) | **8.44s** | 8.76s |
| Advantage | **3.65%** | -- |
| Algorithm | Gourdon (Rust) | Gourdon (C++) |
| Table architecture | b-first BigPiTable | segment-first SegPiTable |
| π lookup cost | ~30-80 ns (L3/DRAM) | ~4 ns (L1) |
| Division cost | ~3 ns (Barrett) | ~18 ns (hardware DIV) |
| Key V8 improvements | build-std +1.3%, large pages +1.3%, CompactPi -1.2% | -- |

V8 compensates for 7.5-20x more expensive π lookups with 6x cheaper divisions, plus micro-architectural tuning that maximizes memory-level parallelism. The 3.65% advantage widened from an initial 1.2% (pre-large-pages) through the accumulation of three infrastructure-level optimizations across Sessions 4-12.

### 5.2 Head-to-Head Comparison

Under identical thermal conditions (5 alternating runs, 45s cooldown, Session 9):

| Metric | V8 | primecount |
|--------|------|-----------|
| Wins | **5/5** | 0/5 |
| Median | **8.589s** | 8.829s |
| Best | **8.546s** | 8.776s |
| Worst | 8.630s | 8.899s |

This controlled comparison was run immediately after enabling 2MB large pages (Session 9). Subsequent sessions confirmed the result: Session 12 achieved 7/7 wins at 4.34% (8.42s vs 8.80s), and Session 14 measured 8.44s vs 8.76s (3.65%). Across all post-large-pages head-to-head comparisons (Sessions 9-14), V8 won every single run.

### 5.3 Statistical Significance

The 3.65% improvement (8.44s vs 8.76s median) far exceeds the ~1.5% thermal variance and warrants statistical confirmation.

**Binomial test (head-to-head wins).** Across 17 controlled head-to-head runs (5 alternating in Session 9, 7 in Session 12, 5 in Session 14), V8 won all 17. Under the null hypothesis (p = 0.5 per run):

P(X = 17 | n=17, p=0.5) = (0.5)^17 = 1/131072 ≈ **7.6 x 10^-6**

This yields **p < 0.00001**, decisively rejecting the null hypothesis. V8's advantage is statistically unambiguous.

**Median comparison.** The sustained medians (8.44s vs 8.76s, delta = 0.32s, 3.65%) exceed the within-tool run-to-run standard deviation (~0.05s for V8, ~0.10s for primecount) by over 3x, providing overwhelming confidence that the difference is systematic.

### 5.4 Scaling Behavior

| Scale | V8 | primecount | Ratio |
|-------|------|-----------|-------|
| 10^12 | 0.006s | 0.014s | **0.4x** |
| 10^14 | 0.018s | 0.023s | **0.8x** |
| 10^16 | 0.167s | 0.178s | **0.9x** |
| 10^18 | 2.27s | 2.27s | **1.0x** |
| 2^63-1 | **8.38s** | 8.72s | **0.96x** |

V8 is faster at all scales except 10^17 (where primecount's SegmentedPiTable advantage peaks for intermediate table sizes).

### 5.5 Parallelization Efficiency

The concurrent penalty analysis reveals the cost of parallelism on shared-memory hardware:

| Component | Solo time | Concurrent time | Penalty | Cause |
|-----------|-----------|----------------|---------|-------|
| AC | 2.1s | 8.23s | 3.9x | L3 contention + DRAM bandwidth sharing with D |
| B | 4.31s | 6.62s | 1.5x | L3 eviction by D, DRAM bandwidth sharing |
| D | 4.50s | 5.94s | 1.3x | Minimal -- streaming access tolerates contention |

Sum of solo times: 2.1 + 4.31 + 4.50 = **10.91s**. Concurrent time: **8.36s**. Parallelization efficiency: 10.91/8.36 = **1.30x speedup** from overlapping three components -- far below the theoretical 10.91/4.50 = 2.42x achievable with independent components. The ~3.9x concurrent penalty on AC (the critical path) means that parallelism costs more than it saves for AC individually, but overall concurrency still wins because B and D are hidden behind AC's inflated time.

This suggests an unusual regime where the critical path's solo performance is excellent (2.10s) but irrelevant -- the system is DRAM-bandwidth-limited, and all three components must share that bandwidth regardless of scheduling.

---

## 6. Implementation Progression: The Journey

Before diving into the lessons, it is useful to understand how the implementation arrived at its current architecture. This is not a story of steady progress. It is a story of walls -- algorithmic walls, hardware walls, correctness walls -- and the sometimes desperate, sometimes inspired decisions that got past them. Rather than starting with Gourdon's algorithm directly, we deliberately implemented progressively more sophisticated algorithms, each one teaching lessons that informed the next. This bottom-up approach mirrors the historical development of the field itself, from Legendre's 1808 formula to Gourdon's 2001 decomposition, and gave us an intuitive understanding of *why* each algorithmic innovation exists that no amount of reading papers could provide.

### 6.1 Version Summary

| Version | Algorithm | Time at Max i64 | Opts (success/total) | Key lesson |
|---------|-----------|-----------------|---------------------|------------|
| V1 | Segmented sieve | >10,000s (est.) | 19/34 (56%) | O(N) is hopeless; L1 sub-segmentation gives 2x |
| V2 | Lucy_Hedgehog | >10,000s (est.) | 10/11 (91%) | Reciprocal tables and 4x unroll for pipeline throughput |
| V3 | Meissel-Lehmer | >1,000s (est.) | 1/6 (17%) | Barrett reduction; pipeline saturated at 3.5 cyc/op |
| V4 | Lagarias-Miller-Odlyzko | 939.21s | 31/39 (79%) | Parallel sieve; adaptive alpha gives 64% at 10^15 |
| V5 | Deleglise-Rivat | ~81s (S2_hard) | 5/6 (83%) | Easy/hard decomposition; parallel S2_hard gives 2.2x |
| V6 | Gourdon (segmented pi) | 342.46s | 5/6 (83%) | ValidM list gives 8.5x fewer iterations |
| V7 | Gourdon (BigPiTable) | 8.65s | ~45/52 (87%) | One big table beats 23K small rebuilds: 39.5x |
| **V8** | **Gourdon (optimized)** | **8.38s** | **3/167+ (2%)** | **321+ experiments confirm hardware floor** |

### 6.2 The Algorithms in Practice

**V1: Segmented Sieve of Eratosthenes.** Every journey needs a starting point, and ours was the most ancient algorithm in number theory: sieve everything, count what survives. At 2^63 this means sieving 9.2 x 10^18 numbers, a computation too large for any single machine. V1 was never going to solve the problem at full scale. But it was a surprisingly rich optimization target: across 34 experiments (19 successful), we discovered wheel-30 factorization (6.7x speedup), L1 sub-segmentation (2.27x -- "the big one" that taught us cache hierarchy matters more than algorithmic cleverness at this level), pre-sieve templates for small primes, Barrett reciprocal fast division, and three-tier prime classification. We also discovered that AVX2 popcount, P-core-only pinning, wheel-210, and 8x unrolling all *failed* -- early warnings of patterns that would repeat at every subsequent version. The sieve code we wrote for V1's inner loop survived, almost unchanged, all the way to V8's D component.

**V2: Lucy_Hedgehog.** Named after its anonymous inventor on the Project Euler forum, this is perhaps the most elegant algorithm in computational number theory: ~100 lines of code implementing an O(x^{3/4} / ln x) dynamic programming approach over the O(sqrt(x)) distinct values of floor(x/k). Simple, beautiful, and remarkably fast for x <= 10^13. V2 was also the version with the highest optimization success rate (10/11, 91%): harmonic block technique (24%), reciprocal tables eliminating all divisions (14%), 4x pipeline unrolling (8.6%), and two-phase iteration (41%). These V2 optimizations taught us the micro-optimization patterns -- reciprocal arithmetic, unrolled pipelines, phase decomposition -- that became foundational techniques in every later version. But the O(x^{3/4}) scaling is merciless: at 10^18 it would take hours. V2 taught us what we call the "sqrt(x) trick": that floor(x/k) takes only O(sqrt(x)) distinct values as k ranges over all integers. This observation, almost trivial once you see it, is the foundation of every faster method.

**V3: Meissel-Lehmer -- and the first wall.** Our first implementation of the classical combinatorial approach, and the version where we discovered Barrett reduction: replacing the hardware `DIV` instruction (25 cycles on Arrow Lake) with a precomputed reciprocal multiply-shift (3 cycles, exact with a single correction multiply). This 8× speedup on the hot-path division became the cornerstone of every subsequent version.

But V3 also taught us what it feels like to hit a wall. After 6 optimization attempts (only 1 succeeded) -- SIMD investigation, 8x unrolling, PGO, allocation elimination -- the u128 multiply throughput saturated at 3.5 cycles per operation. The profiler showed 100% ALU utilization. The 17% success rate (down from V2's 91%) was our first encounter with the optimization success rate collapse that would define V8. The algorithm itself was the bottleneck, and the most promising way forward was a better algorithm. This was a pivotal moment: the decision to abandon a working, heavily-optimized codebase and start over with LMO.

**V4: Lagarias-Miller-Odlyzko -- the first answer.** V4 was the first version to produce the number: pi(2^63 - 1) = 216,289,611,853,439,384. It took 939 seconds. We remember the moment -- 15 minutes of watching a progress bar crawl, followed by the thrill of cross-checking the result against the OEIS.

From there, V4 became the project's most productive optimization laboratory, with 31 successes out of 39 experiments (79% success rate). Parallel phi computation via delta-correction gave 1.8x. A pre-sieve template for small primes gave 1.9x. Adaptive alpha parameters -- discovered by sweeping alpha from 1.5 to 3.0 across input scales -- gave 64% at 10^15. mimalloc replaced the system allocator for an 8.6x reduction in allocation overhead under 24-thread contention. A custom ParallelPiSieve replacing the single-threaded `primal` crate gave 2x. Barrett fast division for easy leaves gave 8%. Branchless cross-off with O(1) count_total gave 8%. The odd-only S2 sieve halved memory and cross-off work. Each optimization peeled away a layer of overhead, revealing the next bottleneck beneath. V4's optimization success rate (79%) was typical of "productive phase" work: the code was fast but far from the hardware floor, and standard techniques reliably improved it.

V4 also produced the project's most insidious bug. Barrett reduction, our prized 8× speedup, turned out to have a subtle overflow: when n × (d − 1) ≥ 2⁶⁴, the reciprocal overestimates the quotient by exactly 1. At 10¹² this never happens. At 10¹⁴ it happens but the errors cancel. At 10¹⁶ the accumulated miscount becomes visible. We discovered it only by cross-validating V2, V3, and V4 at 100 trillion -- three implementations of different algorithms, two agreeing with each other and disagreeing with the third. The fix -- `q - (q*d > n) as u64`, a single conditional subtract -- became permanent across all versions. The lesson became §7.16's Pattern 2: *integer overflows are scale-dependent, and the strongest defense is testing at maximum scale*.

**V5: Deléglise-Rivat -- the bridge.** V5 implemented the easy-leaf/hard-leaf decomposition that is the key insight of modern prime counting: most "special leaves" in the LMO formula can be evaluated by a single table lookup, without touching the sieve at all. By tuning the alpha parameter, work shifts from expensive S₂_hard (requires sieve) to cheap S₂_easy (a single π-table lookup per leaf). V5 was a stepping stone to Gourdon -- it validated the decomposition and taught us the parameter interactions -- but its S₂_hard bottleneck at ~81 seconds proved immovable. Four separate optimization attempts (segment size cap, hierarchical counters, chunk tuning, Fenwick tree) all failed. The algorithm appeared to have reached its architectural limit, just as V3 had reached its pipeline limit.

**V6: Gourdon with segmented π -- the detour that almost worked.** Our first Gourdon implementation followed primecount's architecture faithfully: per-segment SegmentedPiTables of ~3.7 KB, rebuilt for each of ~23,000 segments, ensuring every π lookup hit L1 cache. It achieved 342s at Max i64 -- a 2.7× improvement over V4 that validated Gourdon's formula. But the per-segment rebuild cost scaled poorly, and the S₂_hard bottleneck at ~81 seconds could not be broken.

This was the second pivotal moment. We had implemented primecount's architecture and gotten within 40× of primecount's performance. The conventional path forward was to optimize within this architecture -- better sieve construction, smarter segment scheduling, tighter inner loops. Instead, we asked: *what if the architecture itself is wrong?*

**V7: Gourdon with BigPiTable -- the breakthrough.** The idea was almost too simple: instead of rebuilding a tiny π-table 23,000 times, build one giant table once and look up everything from DRAM. The table would be 285 MB -- far too large for any cache. Every lookup would go to L3 or DRAM at ~50 ns, versus ~4 ns from L1. On paper, this was insane.

It yielded a **39.5× speedup**. 342 seconds → 8.65 seconds.

The insight, which became §7.14 of this paper, is that *amortized rebuild cost dominates latency*. The 23,000 segment rebuilds cost ~50 μs each × 24 threads = 1.15 seconds of pure overhead per thread. Meanwhile, the 4× unrolled inner loop generates 8 independent memory requests in flight simultaneously, hiding most of the DRAM latency through memory-level parallelism. Barrett reduction -- the 3-cycle division from V3 -- made each iteration so cheap that the CPU spent most of its time waiting for memory anyway; switching from 4 ns waits (L1) to 50 ns waits (DRAM) barely mattered when the CPU could issue 8 of them in parallel.

V7 achieved near-parity with primecount for the first time. But the path there required 52 experiments -- nearly as many as V4's entire optimization campaign. The success rate remained high (87%): ValidM lists for 8.5x fewer D iterations, alpha lookup tables for 2.1x at Max i64, primesieve FFI for B (14% faster), streaming merge B (22%), byte-level cross_off_sieve (10.2%), parallel table generation, two-scope early start. But the gap to primecount was only 1.3%, and V7's success rate, while high, was already slightly below V4-V6's averages. The ceiling was near.

**V8: The 167+ experiments -- and the final wall.** Starting from V7's architecture, we spent 14 sessions and approximately 60 hours trying everything we could think of. The success rate collapsed from V7's 87% to **2%**. Of 167+ experiments, only three improved performance: `-Zbuild-std` (recompiling the Rust standard library with `target-cpu=native`, +1.3%), 2MB large pages via mimalloc's `SeLockMemoryPrivilege` (+1.3%), and CompactPi (a two-level pi table reducing memory from 202MB to 51MB, -1.2% wall time). The other 164+ experiments -- software prefetch (7 attempts, all failed), profile-guided optimization (3 attempts, all failed or neutral), data structure compaction (10 experiments, all failed), thread pool reconfiguration (18 experiments, all failed), LLVM flag tuning (8 experiments, all neutral), loop restructuring (6 experiments, all failed), parameter sweeps (12 experiments, all confirming current optimum), and three separate re-implementations of the SegmentedPiTable architecture -- produced the taxonomy in §3 and the constraint model in §4.

The definitive experiment was the streaming AC implementation (Session 14, Opt 154-155): a complete replacement of the 285MB BigPiTable with per-segment L2-cached sieves, correct at all scales, but strictly slower. The discovery that MLP-38 (38 outstanding DRAM loads from a 512-entry ROB) gives effective per-load latency of ~2.6ns -- comparable to L2's ~2.5ns -- proved that the hardware floor is not merely difficult to pass but physically unreachable within the current algorithmic framework. The wall was real, and it had a name: the MLP-MSHR equilibrium.

### 6.3 The Bugs: An Unwelcome Companion

Every version carried bugs, and they followed a pattern that echoes the entire history of prime counting -- from Meissel's miscalculation of π(10⁹) in 1885 (wrong by 56) to Lehmer's π(10¹⁰) in 1959 (wrong by 1). Our 20 bugs are catalogued in §7.16, but three deserve special mention here because they shaped the project's trajectory:

1. **The Barrett overflow** (discovered at V4, present since V3): The optimization that made everything possible -- Barrett reduction, our 8× speedup on the critical division -- was also the source of the most dangerous bug. The reciprocal overestimates by exactly 1 when the input exceeds a scale-dependent threshold. At 10¹² it never triggers. At 100 trillion it triggers twice, and the errors happen to cancel. At 10 quadrillion the accumulated error becomes visible. Finding it required running three independent algorithm implementations on the same input and asking: *why do two agree and one disagree?* The fix was one line of code. The lesson -- that performance optimizations in integer arithmetic can introduce scale-dependent correctness failures -- is universal.

2. **The sieve position-0 bug** (V4): Position 0 in the first sieve segment represents integer 0, which is not in [1, x]. Without explicitly crossing it off, every phi value gets +1 from overcounting. At 100K the error is +28 -- small enough to miss in casual testing. At 1 billion it's +12,999. At 10 trillion it's +2,892,628. The error *grows with x*, meaning the bug is undetectable at the scales where development and debugging happen, and catastrophic at the scales where the code is actually used.

3. **The SegmentedPiTable indexing bug** (V8): Two separate off-by-one errors in the same component -- `(n - low) >> 1` instead of `(n - low - 1) >> 1`, and pi_low = 0 instead of 1 for segment 0. Each was trivial once found. Together they cost a full day of debugging. The pattern -- that segment-0 initialization is a special case deserving its own code path and its own tests -- repeated often enough to become a rule.

### 6.4 What Each Version Taught Us

The progression reveals a pattern: each algorithmic leap provides an order-of-magnitude improvement, while within each algorithm, micro-optimization yields diminishing returns until a hard ceiling is reached. The optimization success rate tells this story quantitatively:

- V1: 34 experiments, 56% success rate -- standard optimization territory
- V2: 11 experiments, 91% success rate -- low-hanging fruit in a new algorithm
- V3: 6 experiments, 17% success rate -- **first wall** (pipeline saturation)
- V4: 39 experiments, 79% success rate -- new algorithm opens fresh territory
- V5-V6: 12 experiments, 83% success rate -- refinement of the approach
- V7: 52 experiments, 87% success rate -- BigPiTable opens massive new space
- V8: 167+ experiments, **2% success rate** -- **final wall** (MLP-MSHR equilibrium)

The V3 dip (17%) was a local wall, broken by switching to a better algorithm. The V8 collapse (2%) appears to be the global wall: no algorithm change within the Gourdon framework can break through it, because the constraint is hardware (DRAM bandwidth, MSHR count, register file), not algorithmic.

The lesson: **the algorithm and data structure architecture determine 99% of performance; micro-optimization determines the last 1%**. But that last 1% is what separates matching primecount from beating it. And the optimization success rate is a leading indicator of which phase you are in: above 50%, standard techniques work and effort is well spent; below 10%, you have reached the floor and further optimization is futile without an architectural change.

And a second lesson, about the process: **the strategic pivots mattered more than the optimizations.** The decisions to abandon V3 for V4, to skip from V5 to Gourdon, and to replace SegmentedPiTable with BigPiTable -- these three human judgments, each requiring the courage to discard working code, provided 99.7% of the total speedup. The 167+ V8 experiments, for all their thoroughness, provided the remaining 0.3%. Knowing *when to change the question* is more valuable than knowing *how to answer it faster*.

---

## 7. Lessons and Discussion

The 321+ experiments produced a result, but they also produced *understanding*. The V1-V7 experiments (154+, ~77% success rate) taught us how optimization works when there is room to improve. The V8 experiments (167+, 2% success rate) taught us what happens when there is not. Each failed experiment was a question answered: *why doesn't this work?* And the answers, taken together, draw a map of the terrain near a hardware performance floor -- terrain that looks very different from the well-explored landscape of "normal" optimization, where profiler-guided improvements reliably yield gains.

This section distills 25 lessons organized into five themes: the nature of the hardware bottleneck (§7.1-7.3), why standard optimizations fail (§7.4-7.9), how the system's components interact (§7.10-7.12), architectural insights (§7.13-7.17), and practical engineering findings (§7.18-7.25). Each lesson is backed by specific experiments; together they paint a picture of what it looks like when you have truly run out of room.

#### Theme 1: The Hardware Bottleneck

### 7.1 Memory-Level Parallelism is the Critical Resource (on This Hardware)

The most important lesson from 321+ experiments is that for large-scale combinatorial computations on modern out-of-order processors, **memory-level parallelism appears to be more important than cache hit rate**. The monotonic sweep experiment (Opt 106) converted 89% of L3/DRAM misses to L1d hits, yet performed 80% slower because serializing iterations reduced MLP from 8 to 1-2 outstanding requests. The streaming AC experiment (Opt 154) went further, building per-segment L2-cached sieves that eliminated the 285MB BigPiTable entirely -- and was still slower, because Arrow Lake's MLP-38 already amortizes DRAM latency to near-L2 levels.

This finding has broad implications: optimization strategies that prioritize cache locality (e.g., cache-oblivious algorithms, loop tiling) may be counterproductive for workloads like ours if they reduce the number of independent memory requests visible to the hardware. Whether this generalizes to all random-access workloads at scale is an open question, but our experiments provide strong evidence for this class of problem.

### 7.2 Software Prefetch is Obsolete on Modern Out-of-Order CPUs

Five separate prefetch experiments (Opt 59, 70, 94, 105) tested distances from 4 iterations (~12 ns) to 128 iterations (~1300 ns ahead). Every single one regressed performance by 3–12%. Arrow Lake's out-of-order engine already looks ahead ~200 instructions, issuing demand loads far earlier than any software prefetch hint. Worse, explicit prefetch instructions consume L2 miss-tracking entries (Line Fill Buffers) that compete with demand loads, *reducing* effective MLP rather than increasing it. This is a strong generalizable finding: on modern processors with deep out-of-order windows (≥200 instructions), software prefetch for random-access patterns is not merely useless -- it is actively harmful.

### 7.3 PGO is Counterproductive for Branchless MLP-Bound Code

Profile-guided optimization was tested three independent times across different sessions (Opt 62, 77, 96), all showing no improvement or slight regression. This contradicts the conventional wisdom that PGO provides "free performance." The explanation is that PGO's primary benefit -- improving branch prediction via layout optimization -- is irrelevant when branches are already >99% predictable (the `is_c2` flag is loop-invariant; `y_boundary_l` mispredicts only once per b-value). PGO's secondary effect -- aggressive function inlining guided by profile hotness -- actually increases L1 instruction cache pressure in the already-tight inner loop. For code where the bottleneck is memory bandwidth and MLP rather than branch misprediction, PGO may be counterproductive.

#### Theme 2: Why Standard Optimizations Fail

### 7.4 Shared-Cache Components Cannot Be Analyzed in Isolation

The DELAY_D experiment (Opt 101) revealed that concurrent components sharing L3 cache are not independent and should not be optimized in isolation. Solo AC completes in ~2.1s, but concurrent AC takes 8.2-8.4s -- a ~3.9x penalty. More surprisingly, *accelerating* AC by removing its overlap with D (DELAY_D scheduling) increased total time because B lost the cache-warming side effect of AC's continuous BigPiTable lookups.

This violates a core assumption of traditional Amdahl's Law analysis: that component times are independent and the total is max(concurrent components). In reality, the components form a coupled system where AC's "wasted" L3 accesses are a free benefit to B. Any analysis that treats component times as independent would conclude that DELAY_D should help -- the opposite of reality. For shared-memory parallel systems with working sets exceeding the last-level cache, component interactions through cache eviction must be modeled explicitly.

### 7.5 Convergence of Radically Different Architectures

V8 and primecount achieve near-identical performance (~8.5s) despite radically different micro-architectural strategies:

| Property | V8 (this work) | primecount |
|----------|----------------|------------|
| π lookup cost | ~30–80 ns (L3/DRAM) | ~4 ns (L1) |
| Division cost | ~3 ns (Barrett) | ~18 ns (hardware DIV) |
| Table size | 285 MB (full-range) | 3.7 KB (per-segment) |
| Parallelism model | b-first, work-stealing | segment-first, ranges |

V8 compensates for 7.5-20x more expensive π lookups with 6x cheaper divisions, while primecount compensates for 6x more expensive divisions with 7.5-20x cheaper L1-resident π lookups. The fact that these radically different tradeoffs converge to within 3.65% of each other is striking, and suggests -- though does not prove -- that the total computation may be approaching a **throughput floor** defined by the minimum number of memory accesses x the minimum cost per access. The streaming AC experiment (Opt 154) strengthens this hypothesis: even eliminating the large table entirely could not break through the floor, because MLP already amortizes the access cost to near-L2 levels.

### 7.6 The Optimization Landscape is Cliff-Edged, Not Plateau-Shaped

Of 167+ V8 experiments, very few produced "slightly worse" results. The distribution is strikingly bimodal: experiments either landed within measurement noise (±1%) or caused >=4% regression, with little in between. This suggests that near-optimal code sits on a **narrow peak** in optimization space, not a broad plateau. Small perturbations in any direction immediately fall off a performance cliff because they disrupt one of the tightly coupled constraints (MLP, register pressure, cache residency). The contrast with earlier versions is instructive: V4's experiments frequently landed in the 2-15% improvement range -- a wide plateau where many directions lead uphill. By V8, the plateau had shrunk to a single point.

This has practical implications for optimization methodology: near an optimum, intuition-guided "small improvements" are overwhelmingly likely to regress. Only exhaustive, systematic exploration -- as in this study -- can confirm that a local optimum has been reached.

### 7.7 Exponentially Diminishing Returns

The implementation's progression from V4 to V8 shows exponentially diminishing returns on optimization effort:

| Transition | Speedup | Experiments (success rate) |
|-----------|---------|--------------------------|
| V4 -> V6 (algorithmic) | 2.7x | ~51 (81%) |
| V6 -> V7 (BigPiTable) | 39.5x | ~52 (87%) |
| V7 -> V8 (167+ experiments) | 1.03x | 167+ (2%) |

The V7->V8 transition invested 167+ experiments for a 3% improvement, while V6->V7 achieved 39.5x from a single architectural change. This exponential decay in returns-per-experiment is characteristic of approaching the hardware's fundamental throughput limit. The success rate collapse (87% -> 2%) quantifies what "diminishing returns" actually looks like in practice.

### 7.8 The Register-Pressure Hard Wall at 16 GPRs

The 8× unrolling experiment (Opt 107) crossed a hard architectural boundary: x86-64's 16 general-purpose registers. The 4× unrolled loop uses 12–14 registers (4 xpq values, 4 pi values, loop counter, accumulators, base pointers) -- near the limit. Doubling to 8× requires 24+ live values, causing stack spills that serialize memory accesses and destroy the MLP advantage that unrolling is meant to provide.

This is not a software limitation but an **ISA constraint**. No compiler optimization or register allocation strategy can create registers that don't exist. Breaking through this wall would require architectures with wider register files -- ARM SVE (32 GPRs), RISC-V (32 GPRs), or future x86 extensions. On current x86-64, our experiments strongly suggest that 4× unrolling is optimal for this access pattern, though we cannot rule out a clever transformation we haven't considered.

### 7.9 Textbook HPC Optimizations Fail Near the Hardware Floor

A striking meta-conclusion emerges from the V8 experiments: **every standard textbook HPC optimization technique was tested and failed**:

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

The only successful optimization was recompiling the Rust standard library (`-Zbuild-std`, Opt 78, +1.3%) -- a compiler infrastructure change, not an algorithmic or micro-architectural one.

This does not mean these techniques are generally useless -- they are well-proven for other workloads. Rather, it demonstrates that **near a hardware-constrained optimum, the standard optimization playbook may be exhausted**. The code appears to be operating at the intersection of multiple hardware limits (MLP, register file, DRAM bandwidth, L3 capacity), and every change we tried that improved one dimension degraded another. This is a cautionary tale: practitioners applying textbook techniques to already-optimized code should expect diminishing (or negative) returns.

#### Theme 3: How Optimizations Interact

### 7.10 Optimizations Destroy Each Other Near the Ceiling

A recurring meta-pattern across the experiments is that **optimizations interact destructively**: adding one optimization destroys the conditions that make another effective.

- **PGO destroys hand-tuned code layout** (Opt 62, 77, 96): The inner loop was manually structured for minimal I-cache footprint with LLVM's `unroll-threshold=800`. PGO's profile-guided inlining and code reordering disrupted this structure, increasing code size and I-cache misses.

- **Prefetch destroys MLP** (Opt 59, 70, 94, 105): Software prefetch instructions consume L2 miss-tracking entries that the 4× unrolled loop's demand loads need. Adding prefetch reduces the number of entries available for the loads that actually produce values.

- **Interleaving destroys spatial locality** (Opt 55, 64, 83, 104): Merging `bits[]` and `prefix[]` arrays halves the number of cache misses per lookup, but doubles the bytes-per-entry, halving the number of bits-words per cache line. Within each b-value's iteration, consecutive xpq values often access nearby bits words, and the dense packing of the separate array provides better sequential prefetch behavior.

- **Branchless restructuring destroys I-cache locality** (Opt 82): Splitting the unified inner loop into six specialized loops (to eliminate branches) increased the code footprint by ~6×, causing I-cache pressure that overwhelmed the branch elimination savings. The original branches were >99% predictable anyway.

- **Thread isolation destroys work-stealing** (Opt 61, 84, 85, 97): Every attempt to give components dedicated thread pools caused catastrophic regression (30–75%). The shared rayon pool's ability to dynamically reassign threads as D's heavy tasks complete is essential -- idle D threads automatically help AC via work-stealing.

This "optimization interference" phenomenon is well-known in compiler optimization (phase-ordering problem), but our experiments demonstrate it at the systems level across hardware resources (cache, registers, miss-tracking entries, threads). Near a hardware-constrained optimum, the optimization space is so tightly packed that any beneficial change in one dimension inevitably degrades another.

### 7.11 Branchless Throughput Beats Branchy Optimization

Across multiple versions and experiments, a consistent finding emerges: **branchless pipeline throughput defeats branch-based optimizations**, even when the branch-based approach does less total work.

- **V3 Phase B transition scanning** (Thought Log): Scanning for value transitions in `small[]` to skip redundant u128 multiplies saved ~27 ms of multiply work but added ~38 ms of branch misprediction overhead. The 23% transition rate made every 4th branch unpredictable, costing 14 cycles each. The original 4× unrolled branchless multiply loop (3.5 cycles/op) was faster despite doing 4× more multiplies.

- **V4 sparse prefix with conditional POPCNT** (Opt 65): Coarse prefix with up to 7 conditional POPCNTs per lookup. The `if block_offset > N` branches generate unpredictable patterns (average 3.5 taken out of 7), costing ~10.5 cycles of misprediction per lookup.

- **V8 branchless loop restructure** (Opt 82): Splitting one loop with two well-predicted branches into six branchless loops increased code size 6× without measurable branch savings.

The lesson: on modern out-of-order processors, a single branch misprediction (~14 cycles) costs as much as 4–7 branchless arithmetic operations. Unless a branch is >99% predictable or eliminates >14 cycles of work per prediction, the branchless path wins.

### 7.12 Compact Encodings with Non-Power-of-2 Strides Are Anti-Patterns

Wheel-30 and mod-240 encodings were tested independently in two different components (V4's S2 sieve and V8's BigPiTable), and both regressed:

- **V4 wheel-30 S2 sieve**: Variable stride (`k += steps[ci]`, cycling through 8 coprime residues) defeated the hardware prefetcher, which tracks constant-stride access patterns. The odd-only sieve's constant stride (`k += p`) allows perfect prefetch prediction. Result: 3–21% regression across scales.

- **V8 wheel-30 BigPiTable** (Opt 56): Index computation requires division by 240 + table lookup for residue class (~4 extra cycles), versus the odd-only encoding's single right-shift (`n >> 7`). At 31.2 billion AC lookups, 4 extra cycles × 31.2B = ~22 seconds of overhead.

The underlying principle: modern CPU memory systems are optimized for power-of-2 addressing. Shift-based indexing (n >> k) compiles to a single instruction with zero latency; division by non-powers-of-2 requires multiply-shift sequences. Hardware prefetchers track linear strides but not modular patterns. **The 47% memory savings from wheel encoding is never worth the indexing overhead** for random-access workloads on current hardware.

#### Theme 4: Architectural Insights

### 7.13 Barrett Reduction as the Architecture Enabler

The BigPiTable architecture's viability depends critically on Barrett reduction (`fast_div`). Without it, the V8 approach would be fundamentally uncompetitive:

- Each AC iteration computes `xpq = x/(p_b × p_l)`, requiring a 64-bit division
- Hardware `DIV` on Arrow Lake: ~18 ns (23 cycles at 5.7 GHz, throughput-limited)
- Barrett `fast_div`: ~3 ns (multiply-high + conditional add, fully pipelined)
- With ~31.2 billion AC lookups, the division savings alone are: 31.2B × 15 ns = **468 seconds**

This 468-second savings dwarfs the BigPiTable's additional latency cost. At ~50 ns average latency penalty vs primecount's L1-resident SegPiTable (over 31.2B lookups = 1,560 seconds penalty), Barrett reduction recovers 30% of that gap. Combined with the elimination of per-segment sieve reconstruction overhead (the other 70%), the two effects together make BigPiTable competitive.

The `fast_div` implementation is exact (not approximate) thanks to a correction step:
```rust
q + (n - q.wrapping_mul(d) >= d) as u64  // branchless correction
```
This correction adds only ~1 cycle (compare + conditional add) but guarantees `fast_div(n, d, recip) == n / d` for all inputs -- critical for algorithmic correctness.

### 7.14 The 39.5× BigPiTable Transition: Why DRAM Can Beat L1

The V6→V7 transition produced the single largest speedup in the project: **39.5×** (342s → 8.65s). This is counterintuitive -- replacing an L1-resident table (~4 ns/lookup) with a 285 MB DRAM-resident table (~50 ns/lookup) should be slower, not faster.

The resolution lies in **amortized per-segment overhead**. V6's segmented approach:
1. For each of ~23,000 segments: rebuild a 3.7 KB SegmentedPiTable (cost: ~50 μs per thread)
2. Process applicable b-values within the segment (fast L1 lookups)
3. Total sieve rebuild cost: 23K segments × 50 μs × 24 threads = **27.6 seconds**

V7's BigPiTable approach:
1. Build the 285 MB table once at startup (0.14 seconds)
2. Process all b-values with random L3/DRAM lookups (no per-segment overhead)
3. Total lookup penalty vs L1: 31.2B × 50 ns = **~1,560 seconds** (but amortized by MLP and parallelism)

With 4× unrolling providing 8 outstanding misses, the effective per-lookup cost drops from 50 ns to ~12 ns (4× MLP). Across 24 cores: 31.2B × 12 ns / 24 = **15.6 seconds** -- plus Barrett saves 468s from V6's hardware divisions. Net: V7 is massively faster because **the one-time table build + MLP-amortized random access is cheaper than 23,000 per-segment sieve rebuilds**.

This insight -- that high-MLP random access to a large table can beat sequential access to many small rebuilt tables -- is the central architectural contribution of this work.

### 7.15 SIMD and Vectorization Are Not Viable for Random-Access Lookups

Early investigation (V3) evaluated AVX-512 and AVX2 for the inner loops:

- **AVX-512 gather** (VPGATHERQQ): loads 4 scattered 64-bit values in ~20 cycles. Scalar loads of the same 4 values take ~4 cycles total (1 cycle each when L1-resident, pipelined). Gather is **5× slower** than scalar because it serializes the address generation and TLB lookups.

- **AVX2 Karatsuba u128 multiply**: ~3 cycles per multiply via `vpmuludq` decomposition. Scalar `mulx` (BMI2): 1 cycle throughput. SIMD is **3× slower** for single multiplies.

- **AVXIFMA** (52-bit fused multiply-add): Available on Arrow Lake, but the gather bottleneck for loading operands from random BigPiTable positions negates any ALU savings.

The fundamental issue: SIMD excels at **uniform operations on contiguous data**. The AC inner loop accesses random, non-contiguous BigPiTable positions determined by `fast_div` results. No SIMD gather instruction on current x86 hardware can compete with scalar loads for random-access patterns, because gather must sequentially resolve TLB entries for each lane.

### 7.16 Correctness Bugs: Taxonomy and Patterns

Across eight versions and 321+ experiments, **20 distinct correctness bugs** were encountered, diagnosed, and fixed. Cataloguing them reveals striking patterns in where errors arise and how they are detected -- patterns that should inform the development of future high-performance number-theoretic code.

#### Bug Category Distribution

| Category | Count | Fraction | Examples |
|----------|-------|----------|----------|
| Off-by-one / boundary errors | 9 | 45% | SegPiTable indexing, sieve position 0, easy/hard boundary gap, phi negative guard, 1-indexed primes, pre-sieve period wraparound, PhiTinyCache OOB, generate_pi unrolled boundary, segment-0 pi_low |
| Integer overflow | 4 | 20% | Barrett reciprocal at 100T+, ParallelPiSieve u32 → u64, V1 fast_div overflow, FactorTable i16 overflow |
| Algorithm misunderstanding | 4 | 20% | small[q] ≠ π(q) during sieve, Type 1 leaves need sieve regardless of xpm, BigPiTable/primes sieve mismatch, skip-3 step pattern |
| Resource explosion | 1 | 5% | 4 GB pi allocation at 1 Quintillion |
| Library API surprise | 1 | 5% | primal::Sieve returns primes beyond requested limit |
| Language semantics | 1 | 5% | Rust release-mode shift-by-64 wraps to shift-by-0 |

#### Pattern 1: Off-by-one errors dominate (45%)

Nearly half of all bugs were boundary/indexing errors. These fall into three sub-patterns:

**Convention mismatches.** The primes array uses a sentinel `primes[0] = 0` with `primes[k]` = k-th prime for k ≥ 1 -- a 1-indexed convention. Code assuming 0-indexed access (`primes[pi_val - 1]`) silently produces wrong results at small scales and catastrophically wrong results at large scales. Similarly, the SegmentedPiTable used `(n - low) >> 1` when BigPiTable uses `(n - 1) >> 1` -- a one-bit difference that corrupted every lookup for even inputs.

**Boundary case neglect.** Several bugs lived at exact boundary values: position 0 in the first sieve segment represents integer 0 (not in [1, x]); the easy/hard leaf decomposition left xpq = y uncovered by either path; the pre-sieve template had a wrapping bug at its period boundary. Each boundary bug is trivial in isolation, but in a system with multiple interacting boundaries (segment edges × prime indices × half-open intervals × integer division rounding), they multiply.

**The "segment-0 is special" trap.** Three separate bugs occurred specifically in segment 0: pi_low must be 1 (not 0) to account for prime 2, which is absent from the odd-only sieve; the sieve itself must explicitly cross off position 0; and the pre-sieve template must handle the first segment's non-aligned start differently. This repeated pattern suggests that segment-0 initialization deserves its own separate code path with dedicated testing.

#### Pattern 2: Integer overflows are scale-dependent (20%)

All four overflow bugs share a characteristic: **they produce correct results at small scales and fail silently at large scales**. The Barrett reciprocal overflows only when n × (d−1) ≥ 2⁶⁴, which at 10T produces 2 errors that happen to cancel, but at 10Q produces 30 errors that accumulate to a visible miscount. The ParallelPiSieve u32 prefix overflows only when π(z) > 4.3 billion -- which occurs at exactly x = 2⁶³−1, the target computation.

The detection method for all four was **cross-validation at escalating scales**: running the same computation at 10¹², 10¹⁴, 10¹⁶, 10¹⁸, and 2⁶³−1, comparing results against independently verified reference values. Bugs that are invisible at 10¹² become glaring at 10¹⁸. This argues for a testing protocol that specifically includes inputs near the maximum supported range, not just representative values.

The Barrett correction deserves special mention: the overestimate *always* gives q+1, never q−1, making the check `q - (q*d > n)` both exact and cheap (a single u64 multiply + compare). This correction became a permanent fixture across all versions -- an optimization that was itself optimized (u128 correction was 20% slower than u64 due to register spills).

#### Pattern 3: Algorithm misunderstandings produce plausible-looking wrong code (20%)

Four bugs arose from misunderstanding the algorithm, not from coding errors:

- **`small[q] ≠ π(q)` during sieve**: During the sieve of Eratosthenes, the intermediate array counts survivors of the *partial* sieve, not π(q). Surviving composites like 25 = 5² (which survives the sieve of {2, 3}) cause the array to change at non-prime positions. This invalidated a prime-batching optimization that assumed constant-between-primes behavior.

- **Type 1 leaves require sieve regardless of xpm > y**: A filter that seemed logically correct (skipping Type 1 leaves where xpm > y) was wrong because *all* Type 1 leaves for b ≤ π(√y) require sieve computation, even when their xpm value exceeds y.

- **BigPiTable and primes array built from different sieves**: The BigPiTable and the primes[] array were constructed by different sieve implementations (parallel segmented vs library), producing slightly different results near boundary values. This only manifested when the two were combined in the clustered-leaves algorithm.

- **Skip-3 cross_off step pattern**: The alternating (p, 2p) step pattern for skipping multiples of 3 had the wrong step order when the starting value was divisible by 3 -- a subtle modular arithmetic error that produced correct-looking results for most primes.

These bugs are particularly dangerous because they pass basic testing. The code "looks right" and produces reasonable-looking numbers. Only systematic cross-validation at multiple scales catches them.

#### Pattern 4: The error-detection hierarchy

In practice, bugs were caught by five mechanisms, roughly in order of how many bugs each caught:

1. **Wrong output at any scale** (caught 12 bugs): The primary defense. Automated regression tests at 15 scales from 10⁴ to 2⁶³−1 caught most bugs immediately.
2. **Cross-validation between versions** (caught 4 bugs): Running V2, V3, and V4 on the same input exposed the Barrett overflow -- V4 was correct (it never used Barrett for the affected operation), while V2/V3 diverged at 100T+.
3. **Crash/panic** (caught 2 bugs): Array bounds violations from PhiTinyCache and FactorTable i16 overflow caused immediate crashes -- the least subtle failure mode.
4. **Performance anomaly investigation** (caught 1 bug): The 4 GB allocation at 1 Quintillion didn't produce wrong results but caused a 10× performance collapse, leading to the discovery of the uncapped allocation.
5. **Debugging optimization failures** (caught 1 bug): The clustered-leaves 4× regression (Opt 95) led to three distinct bug fixes before the algorithm was shown to be *correct but slow* -- the bugs were not causing the regression, but they were hiding in its shadow.

#### Lessons for Number-Theoretic Code

The 20-bug catalog suggests three defensive practices:

1. **Test at maximum supported scale, not just representative scales.** Four overflow bugs and most off-by-one errors are invisible at small inputs. The testing protocol should include inputs that exercise every integer width boundary (2³¹, 2³², 2⁶³, 2⁶⁴).

2. **Cross-validate between independent implementations.** The Barrett overflow was undetectable from any single version's output at moderate scales. Only comparing V2/V3/V4 -- three implementations of different algorithms -- exposed it.

3. **Treat segment 0 as a separate code path.** Three bugs occurred specifically in segment-0 initialization. A dedicated `init_segment_0()` function with its own unit tests would have caught all three.

### 7.17 Adaptive Parameters Are Scale-Dependent

The Gourdon formula has two tuning parameters (α_y, α_z) that control the work distribution among components. A finding consistent across all versions is that **optimal parameters change dramatically with input scale**:

| Scale | Optimal α_y | Optimal α_z | Rationale |
|-------|------------|------------|-----------|
| 10¹² | 2.2 | 1.6 | Small y → fast sieve, many segments for parallelism |
| 10¹⁵ | 6.0 | 2.0 | Larger y amortizes per-prime overhead in S2 |
| 10¹⁸ | 13.0 | 1.75 | Very large y reduces AC iterations |
| 2⁶³−1 | 18.5 | 1.3 | Maximum y to minimize AC (the bottleneck) |

V4's adaptive alpha formula provided a **64% improvement at 10¹⁵** compared to the fixed α = 2.2 optimal at 10¹². This is the largest single optimization gain from parameter tuning in the entire project.

The lesson: combinatorial algorithms with tunable parameters should not use fixed constants. The optimal work distribution depends on the hardware's relative costs for different operations (sieve rebuild, random lookup, division), which shift as the problem scale changes the data structure sizes relative to cache sizes.

#### Theme 5: Practical Engineering

### 7.18 Work-Stealing Is Not Just "Better" -- It Is Essential

Every experiment that isolated thread pools caused catastrophic regression:

| Experiment | Configuration | Regression |
|-----------|--------------|------------|
| Opt 61 | Dedicated AC pool (24 threads) | −65% |
| Opt 84 | Dedicated D pool (16 threads) | −30% |
| Opt 85 | AC limited to 16 threads | −72% |
| Opt 97 | Separate AC + D pools | −75% |
| Opt 90 | P-core affinity for AC (8 threads) | −120% |

The shared rayon pool with work-stealing is not merely a convenience -- it is architecturally essential because the workload has **phase-dependent resource needs**:

1. D's heavy tasks create large rayon work items that may block for milliseconds
2. When D tasks complete, the freed threads immediately help AC via work-stealing
3. B runs in a separate pool but benefits from reduced CPU contention as D winds down
4. The optimal thread-to-component ratio changes continuously during execution

Static thread allocation cannot adapt to this dynamic load. Work-stealing provides automatic, microsecond-granularity load balancing that no manual scheduling we tested could match. This finding generalizes: for concurrent workloads with unequal task granularity and phase-dependent resource needs, **work-stealing thread pools appear to be essential** for achieving acceptable performance -- closer to a correctness requirement than a mere optimization.

### 7.19 The Allocator Matters: mimalloc Under Contention

V4 profiling revealed that memory allocation was consuming 47% of execution time under multi-threaded contention:

- **Windows default heap**: 129 ms allocation overhead across 768 chunks (each allocating ~74 KB)
- **mimalloc**: 15 ms for the same workload -- **8.6× faster**

The root cause: Windows' default allocator uses a global lock for heap operations, serializing allocations across 24 threads. mimalloc uses per-thread heaps with thread-local free lists, eliminating contention entirely.

This finding applies beyond prime counting: **any multi-threaded workload with per-task allocations should use a scalable allocator** (mimalloc, jemalloc, tcmalloc). The default system allocator is often the hidden bottleneck in otherwise well-optimized parallel code.

### 7.20 Rust vs C++ for HPC

Our Rust implementation matches or beats a heavily optimized C++ implementation (primecount) with over a decade of development. Key Rust advantages:
- **Zero-cost abstractions**: Rayon's `par_iter` with work-stealing achieves optimal thread utilization with minimal code complexity.
- **LLVM backend**: Identical code generation quality to Clang, including BMI2 and AVX-512 instruction selection.
- **`build-std`**: Recompiling the standard library with `target-cpu=native` provides a 1.3% improvement unavailable in pre-compiled C++ standard libraries.
- **Safety with escape hatches**: `unsafe` blocks for `get_unchecked` and raw pointer arithmetic in the hot loop, with safe wrappers elsewhere.

### 7.21 The Architecture Ceiling and Future Hardware

The b-first BigPiTable architecture has a proven performance ceiling on this hardware. We established this not merely from theoretical analysis but from **exhaustive empirical evidence** across multiple architectural alternatives:

| Attempt | Configuration | Result | Failure mode |
|---------|--------------|--------|-------------|
| V6 | L2-sized segments (512 KB) | 342s (40x slower) | Per-segment rebuild overhead x 23K segments |
| V8 Opts 86-90 | L1-sized segments (12 KB) | 3-80% regression | Sieve construction O(sqrt(x)) per thread per segment |
| V8 Opts 95-98 | Per-b-value SegPiTable | 4x regression | 42M redundant sieve rebuilds across threads |
| **V8 Opt 154** | **Streaming per-segment sieve** | **6% regression (best)** | **MLP-38 already matches L2 latency; sieve overhead 0.7s** |

The streaming AC experiment (Opt 154, Session 14) was the definitive test. Unlike previous SegPiTable attempts, which suffered from rebuild overhead, the streaming approach built lightweight sieves (~2.4MB, fitting L2) and processed b-values within each segment. It was implemented correctly (verified at 10^15 and Max i64) and tested with three parallelization strategies. All three were slower than the BigPiTable baseline, and the root cause was fundamental: Arrow Lake's MLP-38 (512-entry ROB generating ~38 outstanding DRAM loads) achieves effective per-load latency of ~100ns/38 = 2.6ns, comparable to L2's ~2.5ns. When the hardware already amortizes DRAM latency to near-L2 levels, replacing DRAM access with L2 access provides no benefit while adding sieve construction overhead.

This closes the last remaining avenue for software-level improvement. The SegmentedPiTable approach fails (rebuild overhead exceeds latency savings). The streaming approach fails (MLP already provides L2-equivalent latency). The BigPiTable approach is at its MSHR-saturated limit. All three architectures converge to approximately the same performance, confirming the throughput floor.

**Portability to other architectures.** Our findings are tightly coupled to Intel Arrow Lake's specific characteristics. On different hardware, the equilibrium would shift:

| Architecture | Key difference | Expected impact |
|-------------|---------------|-----------------|
| **AMD Zen 5** | 32 MB L3 (vs 36 MB), different L2 miss handling, 2x SMT | Smaller L3 -> higher miss rate; SMT could improve MLP but adds contention |
| **ARM Neoverse V2** | 32 GPRs, different cache hierarchy, weaker OoO | 32 GPRs could enable 8x unrolling without spills; weaker OoO may reduce effective MLP |
| **Apple M4** | 192-entry ROB, 16 MB shared L2, unified memory | Higher per-core MLP potential; unified memory eliminates DRAM bandwidth wall |
| **Future x86 (APX)** | Potential extension to 32 GPRs | Would break the 16-GPR wall, enabling deeper unrolling |

The MLP constraint model (§4) is portable: on any architecture, the performance ceiling appears to be determined by min(per-core MLP x cores, DRAM bandwidth / bytes-per-lookup, register file / registers-per-iteration). The specific values change, but the analytical framework should apply.

### 7.22 Thermal Effects: The Hidden Variable in Modern Benchmarking

One of the underappreciated realities of performance work on modern processors is that **the CPU you benchmark on the first run is not the same CPU you benchmark on the fifth**. Modern processors aggressively exploit thermal headroom: Intel's Turbo Boost 3.0 can sustain 5.7 GHz on the 285K when the die is cool, but sustained all-core workloads drive temperatures up and frequencies down. Our data quantifies this precisely: cold-CPU best is 8.38s while sustained median is 8.44s -- a **0.7% penalty from thermal throttling** (reduced from 2.1% pre-large-pages, likely because large pages reduce page fault overhead during the thermal-sensitive startup phase).

This has several implications for practitioners:

1. **Cooldown cycles are part of the methodology.** We enforced ≥60 seconds of idle time before cold measurements and monitored core temperatures. Without this discipline, "best" times are artifacts of when the CPU happened to be coolest, not properties of the code.

2. **Alternating runs equalize thermal bias.** Our head-to-head protocol (V8, primecount, V8, primecount, ...) ensures that both tools face similar thermal conditions. Had we run all 10 V8 measurements first, then all 10 primecount measurements, the second tool would systematically appear slower due to accumulated die heating.

3. **Report both cold and sustained numbers.** A researcher reporting only the cold-CPU best is presenting an idealized scenario that users will rarely encounter in production. The sustained median better represents real-world performance. We report both because neither alone tells the complete story.

4. **Thermal variance sets a noise floor for comparisons.** Our 3.65% advantage over primecount is well above the thermal variance and is confirmed by 17/17 head-to-head wins across multiple sessions. Claims of improvement smaller than the thermal variance (~1-2% on this hardware) require extraordinary statistical rigor.

5. **Power-management features interact with workload phases.** During the D→AC→B pipeline, different phases stress different hardware resources -- D is compute-heavy, AC is memory-heavy, B mixes both. Each phase generates different thermal profiles, and the transition between them can trigger frequency adjustments mid-run. This is why our run-to-run variance (~0.05s for V8) is nonzero even on an otherwise-idle machine.

These effects are not unique to prime counting. Any benchmark on a modern boost-clock processor -- from database queries to neural network inference -- faces the same thermal confound. The broader lesson: **on modern processors, performance is a function of temperature as much as code quality**, and benchmarking methodology must account for this.

### 7.23 Limitations

Our comparison with primecount uses wall-clock time under controlled conditions but does not account for potential platform-specific advantages (primecount may perform differently on AMD processors or Linux systems). The thermal effects discussed in §7.22 are mitigated by our alternating-run protocol but remain a source of systematic uncertainty.

**Energy efficiency.** We do not measure power consumption or energy-per-computation. The BigPiTable architecture's heavy DRAM traffic (estimated ~50–70 GB/s sustained during AC) likely consumes significantly more memory-subsystem power than primecount's L1-resident approach. An energy-efficiency comparison (joules per π(x) computation) could favor primecount despite its slower wall-clock time.

**Single-platform evaluation.** All results are from a single machine running Windows 11. Linux kernel scheduling, different NUMA topologies, and different memory controllers could shift the balance. The 3.65% advantage is large enough that it is unlikely to reverse on the same hardware under different OS conditions, but cross-platform generalization requires further testing.

---

## 8. Related Work

### 8.1 Algorithms and Theory

**Legendre (1808)**: Proposed the first combinatorial formula for π(x) using inclusion-exclusion over prime factors, requiring O(x) operations. **Meissel (1870, 1885)**: Reduced the problem to O(x^{2/3}/ln x) by partitioning integers by their largest prime factor, computing π(10⁹) by hand -- a result later found to be off by 56, discovered 73 years later by Lehmer [7]. **Lehmer (1959)**: Extended Meissel's method and computed π(10¹⁰) on an IBM 701 -- also wrong by 1, not corrected until 1986 [8]. These early errors underscore the algorithmic and numerical complexity of the problem.

**Lagarias, Miller, and Odlyzko (1985)** [5]: The LMO method reduces complexity to O(x^{2/3}/ln x) with O(x^{1/3+ε}) space, enabling practical computation up to 10¹³. Our V4 implements LMO with parallel phi computation and Barrett reduction. **Lagarias and Odlyzko (1987)** [9] also proposed an analytic method computing π(x) in O(x^{1/2+ε}) time via numerical integration over the Riemann zeta function's zeros, but the enormous constants make it impractical below ~10²³.

**Deléglise and Rivat (1996)** [4]: Introduced the easy-leaf/hard-leaf decomposition (S₂_easy vs S₂_hard) that shifts most work from expensive sieve computation to cheap table lookups. Our V5 implements this directly and demonstrated the decomposition's power: by tuning the α parameter, S₂_hard's share of total work can be minimized.

**Gourdon (2001)** [1]: Extended the Deléglise-Rivat approach with a different formula decomposition (AC, B, D components rather than S₂_easy/S₂_hard) that enables finer-grained parallelization. Our V6–V8 implement Gourdon's algorithm, and V8 is the primary subject of this paper.

**Platt (2012)** [10]: Applied the analytic method to verify π(10²⁵), using the Odlyzko-Schönhage algorithm for zeta function evaluation on high-performance cluster hardware. The analytic method remains the only approach that scales sub-linearly in time with x, but its practical crossover point with combinatorial methods lies well beyond 10²⁰.

### 8.2 Implementations

**primecount (Walisch, 2010-present)** [2]: The primary benchmark target for this work. primecount implements both Deleglise-Rivat and Gourdon's algorithms in C++, with OpenMP parallelization. Its architecture centers on a **SegmentedPiTable** -- a per-segment sieve of ~3.7 KB that fits in L1 cache, rebuilt for each of ~23,000 segments. This gives O(1) π lookups with ~4 ns latency, at the cost of per-segment reconstruction overhead (~50 μs each). primecount uses hardware integer division and `libdivide` for the AC inner loop. Its scheduling is sequential: AC first (using all threads), then B, then D -- avoiding the shared-cache contention that plagues concurrent approaches. We tested primecount v8.2 on identical hardware (8.72s best, 8.76s median), providing the controlled comparison reported in §5.

**primesieve (Walisch, 2010–present)** [3]: A highly optimized segmented sieve of Eratosthenes used by both primecount and our implementation for B-component prime enumeration. primesieve achieves near-memory-bandwidth speeds through cache-optimal segmentation, bucket sorting of large primes, and SIMD-optimized counting. Our V8 uses primesieve's C API (via FFI) for streaming prime generation in the B component.

**Lucy_Hedgehog algorithm (2012)**: A remarkably concise O(x^{3/4}/ln x) method popularized on the Project Euler forum [11]. It computes π(x) using a dynamic programming approach over the O(√x) distinct values of ⌊x/k⌋, requiring only ~100 lines of code. Our V2 implements this algorithm, and it inspired the "√x trick" -- the observation that only O(√x) distinct quotients exist -- which underlies all faster combinatorial methods. The algorithm is competitive up to ~10¹³ but scales too slowly for larger inputs.

**Computational records**: The largest verified prime counting computation is π(10²⁹) = 1,520,698,109,714,272,166,094,258,063 by David Baugh and Kim Walisch (2022) using primecount on high-memory cluster hardware [12]. These record computations use the Deléglise-Rivat variant rather than Gourdon's, as DR has better space efficiency at extreme scales.

### 8.3 This Work in Context

Our contribution is not a new algorithm but rather a new **architectural tradeoff**: replacing primecount's segment-first approach (L1-resident SegmentedPiTable with hardware division) with a b-first approach (285 MB BigPiTable with Barrett reduction and high MLP). The two approaches represent opposite ends of a design spectrum -- ours sacrifices cache hit rate for MLP and division speed, while primecount sacrifices division speed and MLP for cache locality. That both converge to within 3.65% of each other (§7.5) is suggestive evidence that the total computation may be approaching a throughput floor. The streaming AC experiment (§7.21) provides the strongest evidence yet: a third architectural approach (per-segment L2-cached sieves) also converges to approximately the same performance, suggesting the floor is real and architecture-independent.

Notably, we actually implemented and benchmarked primecount's SegmentedPiTable approach three separate times (V6, V8 Opts 86–90, V8 Opts 95–98), each with different parameters (L1-sized to L2-sized segments). All three attempts were slower than the BigPiTable architecture, ranging from 3% to 80% regression. The failure is not because SegmentedPiTable is a bad idea -- it is excellent in primecount -- but because the rebuild cost per segment (~50 μs × 23,000 segments × redundant work across threads) exceeds the latency savings from L1-resident lookups when combined with our Barrett reduction and 4× unrolled MLP pipeline. This empirical finding contradicts the intuition that "closer to the CPU is always faster."

---

## 9. Conclusion

We set out to answer a simple question -- *how fast can you count the primes below 2^63?* -- and ended up answering a more interesting one: *what does the approach to a hardware floor look like, and how do you know when you have arrived?*

The specific answer: 8.38 seconds on Intel Arrow Lake, beating the long-standing C++ state-of-the-art by 3.65% (p < 0.00001 by binomial test on 17 head-to-head runs). But the general answer is what we hope readers will remember.

Through 321+ optimization experiments across 8 algorithm versions, we mapped the full trajectory from "code that can be improved" to "code that cannot." The optimization success rate tells this story quantitatively: 56-91% in V1-V2 (fertile territory), 79-87% in V4-V7 (productive refinement), 17% in V3 (a local wall broken by algorithm change), and 2% in V8 (the global wall). The collapse from 87% to 2% between V7 and V8 is the empirical signature of the hardware floor. The streaming AC experiment (Session 14) provided the definitive proof: even eliminating the 285MB BigPiTable entirely -- replacing it with L2-cached per-segment sieves -- could not improve performance, because Arrow Lake's MLP-38 already amortizes DRAM latency to near-L2 levels.

The journey from V1 to V8 taught us that **the returns on effort are spectacularly non-linear**: three strategic decisions (switching from Meissel-Lehmer to LMO, adopting Gourdon's decomposition, and replacing SegmentedPiTable with BigPiTable) provided 99.7% of the speedup. The V1-V7 optimization campaigns (~154 experiments, ~77% average success rate) provided the foundation. The 167+ V8 experiments, consuming most of the project's time, provided the last 0.3% and the proof that the floor had been reached.

Beyond the specific result, this study yields generalizable findings across five categories:

**Hardware constraints:**
1. **MLP over cache hit rate**: Memory-level parallelism governs performance for random-access workloads; optimizations that improve locality at the cost of MLP are counterproductive -- including eliminating the table entirely (§7.1).
2. **The 16-GPR wall**: x86-64's register file is the binding constraint on unroll factor for MLP-generating loops; wider ISAs (ARM, RISC-V with 32 GPRs) could potentially push further (§7.8).
3. **MSHR saturation as the true ceiling**: With ~38 of ~48 MSHRs occupied, the system has no capacity for additional memory requests regardless of software technique (§4.1).

**Anti-patterns near the optimum:**
4. **Software prefetch is harmful** on processors with deep OoO windows (§7.2).
5. **PGO fails for MLP-bound code** -- inlining increases I-cache pressure (§7.3).
6. **Optimizations destroy each other** -- PGO vs hand-tuning, prefetch vs MLP, interleaving vs spatial locality (§7.10).
7. **Textbook techniques fail at the floor** -- every standard HPC optimization was tested and failed (§7.9).

**Systems-level insights:**
8. **Shared-cache coupling** invalidates independent component analysis (§7.4).
9. **Work-stealing is essential**, not optional, for phase-dependent concurrent workloads (§7.18).
10. **The allocator matters** -- mimalloc was 8.6x faster than the default heap under contention (§7.19).
11. **The success rate collapse is a diagnostic**: when optimization success rate drops below ~10%, you have reached the hardware floor (§3.0, §6.4).

**Architectural insights:**
12. **DRAM can beat L1** -- high-MLP random access to a large table beats sequential access to many small rebuilt tables (§7.14).
13. **Three architectures converge** -- BigPiTable, SegmentedPiTable, and streaming sieves all reach approximately the same performance, confirming the throughput floor is architecture-independent (§7.21).
14. **Barrett reduction enables the architecture** -- 6x faster division compensates for 20x slower π lookups (§7.13).

**On correctness:**
15. **Off-by-one errors are the dominant failure mode** in number-theoretic code -- 45% of our 20 bugs -- and they are invisible at small scales (§7.16).

**On methodology:**
16. **Thermal effects are the hidden variable** -- modern boost clocks create a measurable performance gap between cold and sustained runs (§7.22).

The MLP constraint model (§4) provides a portable analytical framework: on any architecture, the performance ceiling appears to be determined by the minimum of MSHR capacity, system DRAM bandwidth, and register file depth. We believe this framework applies beyond prime counting to hash tables, database joins, graph traversals, and any random-access workload at scale where the working set exceeds the last-level cache.

Finally, a note on method. This work was produced by a human-AI collaboration in approximately 120 hours of interactive sessions spanning 14 sessions over several weeks. The AI's contribution -- implementing 321+ experiments with full documentation of every success and failure -- would have taken a solo researcher many months. The human's contribution -- the three strategic pivots that provided 99.7% of the speedup -- could not have come from the AI. Critically, this collaboration was enabled by persistent session infrastructure: a chronicle database that maintained context across sessions interrupted by power failures, context limits, and multi-day gaps. Without this persistence layer, the systematic search through 167+ V8 experiments -- each building on findings from earlier sessions -- would have been impractical. The AI could query its own history ("what prefetch distances were already tested?", "what was the AC solo time in Session 8?") and avoid repeating failed experiments, enabling a level of systematic rigor that neither a solo human (limited by memory and fatigue) nor a stateless AI (limited by context windows) could achieve alone. The complete experiment logs, thought journal, and source code are available at https://github.com/secwest/fast-prime.

### If You Remember Nothing Else

> **For practitioners**: Near a hardware-constrained optimum, standard optimization techniques (prefetch, PGO, cache tiling, loop transforms) don't just fail -- they make things worse. Watch your optimization success rate: when it drops below 10%, you have reached the floor.
>
> **For systems researchers**: Memory-level parallelism is the critical resource for random-access workloads on modern out-of-order CPUs. Optimizing for cache hit rate at the expense of MLP is a trap -- an 80% regression from "better" locality is not a theoretical concern but an empirical result. Even eliminating the large table entirely cannot help when MLP already amortizes DRAM latency to near-L2 levels.
>
> **For language designers**: Rust's performance matches C++ for memory-bound HPC, with `build-std` providing a unique advantage. The safety/performance tradeoff is not a tradeoff -- `unsafe` blocks in hot paths with safe wrappers elsewhere gives both.
>
> **For anyone working with AI**: The stamina is the AI's gift -- 321+ experiments, each fully documented. The wisdom is the human's -- knowing when to stop optimizing and start over. Persistent session infrastructure makes the difference between a stateless assistant and a research partner that builds on its own prior work. Together, they got further than either could alone.

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

## Appendix B: Component Timing at pi(2^63 - 1)

| Mode | AC | B | D | Total |
|------|------|------|------|-------|
| All concurrent (default, Session 14) | 8.23s | 6.62s | 5.94s | 8.36s |
| AC alone (PHASE_AC_DB) | **~2.1s** | -- | -- | -- |
| B alone | -- | **4.31s** | -- | -- |
| D alone (PHASE_D_ACB) | -- | -- | **4.50s** | -- |
| D on separate pool (D_THREADS=8) | **3.1s** | 4.9s | 23.9s | 24.2s |

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
| 66 | B_THREADS=4 analysis | 14.36s | -- |
| 67 | Phase scheduling | 9.06s | −3.9% |
| 68 | Dedicated D pool | 8.71s | ≈0 |
| 69 | D chunk granularity | 8.66s | ≈0 |
| 70 | Same-batch prefetch | 8.97s | −4.3% |
| 71 | PGO (nightly, blocked) | -- | -- |
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
| 106 | Monotonic sweep running pi | 15.62s | -80% |
| 107 | 8x unrolling | 8.93s | -4% |
| **108** | **2MB large pages (mimalloc)** | **8.60s** | **+1.3%** |
| 109 | D prefix-sum counter array | 19.3s | -125% |
| 110 | B chunk count reduction | 8.55s | ~0 |
| 114 | AC narrow b-value reorder | 8.57s | ~0 |
| 117 | BigPiTable two-level prefix | 8.89s | -3.5% |
| 118 | AC 8x unroll | 8.97s | -4.4% |
| 119 | D Type 2 work estimator | 8.58s | ~0 |
| 121 | LLVM flags (misched, inline) | 8.56s | ~0 |
| 122 | Wheel-30 BigPiTable | 9.67s | -13% |
| 124 | P-core/E-core affinity | 15.0s | -75% |
| 125 | Interleaved BigPiTable v5 | 8.80s | -2.7% |
| 126 | Rayon min_len tuning | 8.92s | -4% |
| 127 | Process priority HIGH/REALTIME | 8.63s | ~0 |
| 128 | B thread throttling | 8.89s | -3.7% |
| 129 | 1GB huge pages | 8.55s | ~0 |
| 130 | LTO variations | 8.60s | ~0 |
| **135** | **CompactPi two-level pi table** | **8.42s** | **-1.2%** |
| 136 | B_THREADS throttling | 8.58s | ~0 |
| 137 | Split-loop AC (hoist branches) | 9.29s | -8% |
| 138 | AC_SEG tuning | 8.42s | ~0 |
| 139 | B_CHUNKS tuning | 8.42s | ~0 |
| 140 | D_THREADS isolation | 11.1s | -32% |
| 141 | Phase scheduling (sequential) | 8.95s | -6% |
| 142 | Alpha_y tuning | 8.42s | ~0 |
| 143 | Alpha_z tuning | 8.41s | ~0 |
| 144 | D_SEG_CAP tuning | 9.25s | -10% |
| 145 | Non-temporal B scan (NTA) | 8.43s | ~0 |
| 146 | Verify div/mod 30 codegen | -- | confirmed |
| 147 | Reduce ValidM struct | -- | not viable |
| 148 | Alternative allocator (snmalloc) | 8.56s | -1.6% |
| 149 | B start delay | 8.47s | ~0 |
| 150 | AC segment order | 8.54s | -1.2% |
| 151 | Hot loop disassembly audit | -- | confirmed optimal |
| 152 | Speculative prefetch analysis | -- | not viable (MSHR) |
| 153 | 8x unroll analysis | -- | not viable (MLP invariant) |
| 154 | **Streaming AC (StreamSieve)** | 8.90s | -6% (best strategy) |
| 155 | Streaming AC parallelization | 15.84s | -88% (dedicated pool) |
| 156 | Alpha parameter sweep | 8.49s | ~0 |

Of 167+ V8 experiments, **3 succeeded** (Opt 78: build-std +1.3%, Opt 108: large pages +1.3%, Opt 135: CompactPi -1.2%), **~15 were neutral** (within noise), and **~149 were regressions** (ranging from -0.6% to -8.7x). For the full optimization history across all 8 versions (321+ experiments), see the OPTIMIZATIONS_V*.md files.

---

## Acknowledgments

### On Human-AI Collaboration

A word about how this work was done, because the method is part of the story.

This paper documents the result of a collaboration between a human (Dragos Ruiu, a security researcher) and an AI (GitHub Copilot CLI, powered by Claude). The human had systems programming experience and deep hardware intuition, but no prior work in analytic number theory. The AI had broad knowledge of algorithms and the ability to write, compile, benchmark, and analyze code in rapid cycles -- but no ability to feel frustrated, get tired, or have the flash of insight that says *the whole approach is wrong, start over*.

The workflow looked like this: the human would say something like "let's try Barrett reduction for the division" or "this bottleneck pattern means we need a completely different algorithm." The AI would implement the change, build it, run benchmarks, analyze the results, update the documentation, and present findings -- typically completing the full cycle in under 10 minutes. If the experiment failed (as most of the 321+ did), the AI would revert the code, document the failure mechanism, and propose the next experiment. If the experiment produced wrong results, the AI would debug, stepping through the logic until the bug was found and fixed.

This division of labor turned out to be remarkably productive. The mechanical cycle -- edit, build, benchmark, analyze, revert, document -- is where human researchers lose time and motivation. Running 321+ experiments manually, with careful documentation of each, would take a solo researcher many months and generate far less complete records. The AI maintained perfect documentation throughout: every experiment logged with its hypothesis, result, and failure mechanism. This paper's §3, §6, and §7 are drawn directly from those logs.

But the strategic decisions -- the ones that produced 99.7% of the speedup -- were human. The decision to abandon V3's optimized Meissel-Lehmer for V4's LMO. The decision to implement Gourdon instead of continuing to optimize Deleglise-Rivat. The wild idea to replace 23,000 L1-cached SegmentedPiTables with a single 285 MB DRAM-resident BigPiTable. Each of these required the willingness to throw away working code based on a hunch -- something that current AI systems, which optimize locally within a given framework, do not do on their own.

### The Chronicle: Session Persistence as Research Infrastructure

The collaboration faced a fundamental challenge: context limits. Each interactive session could hold only a fraction of the project's accumulated knowledge (3,900+ lines of thought log, 1,600+ lines of optimization log, 2,500+ lines of implementation code). Power failures, session timeouts, and context window limits interrupted work repeatedly. Early in the project, this meant painstaking manual recovery from git history and log files.

The turning point came with the introduction of a **persistent session database** -- a SQLite-backed chronicle that maintained structured records across sessions: prior checkpoints with technical details, experiment results, file change history, and searchable full-text indexes of all prior conversation. This infrastructure transformed the collaboration in several ways:

1. **Avoiding redundant work.** When Session 14 set out to implement streaming AC, the AI could query its own history to find that per-segment SegPiTable had already been tried three times (V6, Opts 86-90, Opts 95-98) with known failure modes. This prevented a fourth attempt at the same dead end and instead guided the design toward a fundamentally different streaming approach.

2. **Building on prior quantitative findings.** The MSHR saturation analysis in Session 13 (Opt 151-152) depended on disassembly data and MLP calculations from Session 11's ROB-limited-MLP discovery. Without persistent session state, these connections would have been lost across context boundaries.

3. **Enabling systematic search.** The 167+ V8 experiments were not random explorations but a systematic search guided by a shrinking list of untried strategies, maintained across sessions. Each session could query "what hasn't been tried?" and "what failed for what reason?" to avoid duplication and target the remaining gaps.

4. **Preserving institutional memory.** The success rate collapse data in §3.0 and §6.4 -- covering all 321+ experiments across 8 versions -- could only be computed because the chronicle maintained records from the earliest sessions. A stateless system would have no access to V1-V7 optimization history when analyzing V8 results.

This experience suggests that **persistent session infrastructure is not a convenience but a prerequisite** for AI-assisted research projects that span multiple sessions. The alternative -- relying on the human to maintain and re-inject context manually -- scales poorly beyond ~50 experiments. The chronicle database enabled a level of systematic rigor that neither a solo human (limited by memory) nor a stateless AI (limited by context windows) could achieve alone.

The aggressive commit-and-log discipline described in §2.5 -- committing after every experiment, maintaining a 3,900-line thought log -- was born from early pain with session loss. It turned out to be excellent research methodology regardless: the logs became the raw material for this paper, and the chronicle made them queryable.

**What we learned about human-AI research collaboration:**
- **The AI excels at breadth**: 321+ experiments in ~120 hours, each fully documented. A human would have run 40-60 before fatigue set in.
- **The human excels at depth**: The three strategic pivots (V3->V4, V5->V6, V6->V7) provided 99.7% of the speedup. The AI's experiments provided 0.3% -- but also the proof that 0.3% was all that remained.
- **Documentation becomes free**: When the AI maintains logs as a natural part of its workflow, the paper practically writes itself. This paper's 25 Discussion subsections were drawn from experiment logs that existed before the paper was conceived.
- **Session persistence enables long-term strategy**: Without the chronicle, V8's systematic exhaustion of the optimization space would have degenerated into a random walk. With it, each session could build on all prior sessions, making the multi-week research arc coherent.
- **The revert discipline is essential**: With an AI that can implement changes quickly, the temptation is to accumulate changes. The discipline of reverting every failed experiment -- returning to a clean baseline before the next attempt -- was critical for experiment independence and for maintaining sanity.

We offer this account not as a claim that AI replaces human researchers, but as evidence that human-AI collaboration, augmented by persistent session infrastructure, can produce research that neither could have done alone in the same timeframe. The resulting documentation may actually be *better* than what a solo researcher would produce, because the AI has no incentive to skip the tedious step of writing down why something failed, and the chronicle ensures that nothing is forgotten.

### Technical Acknowledgments

This work used Kim Walisch's *primesieve* library [3] for B-component prime enumeration and *primecount* [2] as both the primary benchmark target and a source of algorithmic insight -- our study of primecount's SegmentedPiTable and sequential scheduling architecture directly informed the design decisions in §6 and the comparative analysis in §7.5. The implementation builds on the Rayon parallel runtime for work-stealing thread scheduling and the mimalloc allocator for contention-free parallel allocation.

## References

1. Gourdon, X. (2001). "Computation of pi(x): improvements to the Meissel, Lehmer, Lagarias, Miller, Odlyzko, Deleglise and Rivat method."
2. Walisch, K. (2010–present). *primecount*: Highly optimized C++ implementation of the prime counting function. https://github.com/kimwalisch/primecount
3. Walisch, K. (2010–present). *primesieve*: Fast prime number generator. https://github.com/kimwalisch/primesieve
4. Deléglise, M. and Rivat, J. (1996). "Computing π(x): The Meissel, Lehmer, Lagarias, Miller, Odlyzko method." *Mathematics of Computation*, 65(213), 235–245.
5. Lagarias, J.C., Miller, V.S., and Odlyzko, A.M. (1985). "Computing π(x): An analytic method." *Journal of Algorithms*, 6(3), 537–560.
6. Oliveira e Silva, T. (2006). "Computing π(x): the combinatorial method." *Revista do DETUA*, 4(6), 759–768.
7. Lehmer, D.H. (1959). "On the exact number of primes less than a given limit." *Illinois Journal of Mathematics*, 3(3), 381–388.
8. Meissel, E. (1885). "Berechnung der Menge von Primzahlen, welche innerhalb der ersten Milliarde naturlicher Zahlen vorkommen." *Mathematische Annalen*, 25, 251–257.
9. Lagarias, J.C. and Odlyzko, A.M. (1987). "Computing π(x): an analytic method." *Journal of Algorithms*, 8(2), 173–191.
10. Platt, D.J. (2012). "Computing π(x) analytically." *Mathematics of Computation*, 84(293), 1521–1535.
11. Lucy_Hedgehog (2012). "Counting primes with a simple sieve." Project Euler forum thread #10.
12. Baugh, D. and Walisch, K. (2022). "Computation of π(10²⁹)." https://github.com/kimwalisch/primecount/blob/master/doc/Records.md

---

*Source code: https://github.com/secwest/fast-prime*
