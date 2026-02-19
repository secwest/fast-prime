# Optimization Log — V3 (Meissel-Lehmer Prime Counting)

Algorithm: Meissel-Lehmer extension of Lucy_Hedgehog.
Sieve primes only up to N^{1/3}, compute remaining P₂ contribution analytically.
Complexity: O(N^{2/3}) time, O(√N) space.

Hardware: **Intel Core Ultra 9 285K** (Arrow Lake, 8P+16E, 24 threads).
All times best-of-5+, release mode, `lto=fat`, `codegen-units=1`, `target-cpu=native`.

---

## Baseline: Meissel-Lehmer with V2 Optimizations

Started from V2's fully optimized Lucy_Hedgehog (reciprocal table, two-phase harmonic,
4× unroll, Barrett fast division) and added:

- **Meissel truncation**: Stop the Lucy_Hedgehog sieve at N^{1/3} instead of N^{1/2}
- **P₂ analytic sum**: For primes p > N^{1/3}, compute π(n) contribution as
  `Σ [large[p] - π(p-1)]` — just one array lookup per prime, no inner loops.

Key insight: for p > N^{1/3}, the S_a(n/p) values are frozen after the partial sieve
because any modifying prime q > N^{1/3} has q² > N^{2/3} > n/p, so j_end(q) < p.
The j > 1 contributions don't propagate to large[1] because subsequent primes'
Branch 1 reads from positions > j_end (which are unmodified).

| Range | V2 Time | V3 Time | Speedup |
|---|---|---|---|
| 1 Billion | 0.002s | 0.002s | — |
| 10 Billion | 0.007s | 0.007s | — |
| 100 Billion | 0.035s | 0.034s | 3% |
| 1 Trillion | 0.176s | 0.170s | **3.4%** |
| 10 Trillion | 1.230s | 1.190s | **3.3%** |

---

## Optimization #1: Eliminate all_primes Vec allocation

**Result: 1T 0.170→0.168s (1.2%)**

Replaced pre-collected `all_primes` vector (~78K elements at 1T) with an on-demand
iterator from the primal sieve for the P₂ computation. Avoids one large Vec allocation
and initial skip-iteration over already-processed sieve primes.

---

## Profiling Analysis (1T / 10T)

Internal timing breakdown at 1T (N=10^12, v=10^6, 1229 sieve primes):

| Phase | Time (1T) | % | Time (10T) | % |
|---|---|---|---|---|
| Init (arrays + reciprocal table) | 5.8ms | 3.1% | 17.8ms | 1.4% |
| Large update (Branch 1 + Phase A + Phase B) | 151.4ms | **81.3%** | 1028ms | **82.3%** |
| Small update (168 primes with p ≤ √v) | 27.6ms | 14.8% | 199ms | 16.0% |
| P₂ sum | <1ms | <1% | <1ms | <1% |

**Key finding**: The large update dominates (~81%), NOT the small update (~15%).
This is the opposite of what was assumed. The bottleneck is the u128 reciprocal
multiply in Phase A/B, running at ~3.5 cycles/op (close to the hardware minimum
of 3 cycles for MUL r64→r128 on Arrow Lake).

---

## Failed Optimization Attempts

### Fused init loops (3→1)
Combined small[], large[], recip[] initialization into a single loop.
**Result**: Neutral — separate sequential passes are better because each has a
different memory access pattern optimized by the CPU's prefetcher.

### Pre-sieve p=2 (skip first sieve pass)
Initialize arrays to post-p=2 values: small[j] = ⌈j/2⌉, large[j] = ⌈(n/j)/2⌉.
**Result**: 1T **regressed** 1.8% (0.170→0.173s). The more complex initialization
(`(j+1)>>1` vs `j-1`) slightly slows the init loop, and the p=2 sieve pass is
already fast (~2ms, <1.2% of runtime).

### Software prefetching for Phase A (distances +4, +16, +32)
Added `_mm_prefetch` for small[] lookups in the Phase A 4× unrolled loop.
- +4: Too close, no benefit. Extra u128 multiply adds overhead.
- +16: 10T improved 1.2%, 1T within noise. Extra multiply cancels gains.
- +32: Similar to +16. L3 latency (~40 cycles) partially hidden but
  the overhead of the prefetch multiply negates the cache benefit.
**Result**: Neutral overall — Arrow Lake's hardware prefetcher handles the
gradually-changing stride pattern adequately.

### Zero-cost prefetch (linear extrapolation)
After computing q0-q3, extrapolated next batch's address: `pf_q = q3 - (q0 - q3)`.
**Result**: 1T **regressed** 4.7% (0.170→0.178s). The prefetch is issued too late
(after all 4 multiplies complete) and the extrapolated address is not accurate
enough, causing spurious cache line loads.

### Fast reciprocal via f64 approximation
Replace u128 division `(1u128 << 64) / j` with `(2^64_f64 / j as f64)` + u128
verification.
**Result**: **Incorrect** — f64 has only 53-bit mantissa. For reciprocal values
~10^18, the ULP is ~512, causing off-by-hundreds errors in the reciprocal table.
1M case produced wrong answer (78497 instead of 78498).

### Extended Meissel-Lehmer (N^{1/4} cutoff)
Analysis: sieving to N^{1/4} instead of N^{1/3} would save ~16ms at 1T by
eliminating 1061 primes from the sieve. But the P₂ correction (computing
π(N/p_k) for ~78K primes via semiprime subtraction) requires ~10M small[]
lookups at 1T (~9ms). For 10T, the correction costs ~91ms vs ~73ms savings.
**Result**: Not implemented — computational cost of P₃ + P₂ correction
exceeds the sieve savings, especially at 10T. Would require the full
Deleglise-Rivat method for a net benefit.

---

## Analysis: Why V3 is Near-Optimal

The large update runs at ~3.5 cycles per operation on Arrow Lake:
- **MUL r64→r128**: 3-cycle latency, 1-cycle throughput
- **Address-dependent load** from small[q]: must wait for multiply result
- **OoO execution** with 4× unroll hides some latency but can't fully overlap
  the multiply→load dependency chain

The theoretical minimum is ~3 cycles/op (multiply throughput bound).
The 0.5 extra cycles come from L2/L3 cache misses on the random small[] reads.

For a fundamentally faster approach, the algorithm would need to be changed
to one with lower complexity (e.g., Deleglise-Rivat at O(N^{2/3}/ln²N)),
which is significantly more complex to implement correctly.

---

## Current Best (V3)

| Range | Time | vs Sieve V1 (24 threads) | vs V2 |
|---|---|---|---|
| 1 Billion | 0.002s | 3.0× faster | same |
| 10 Billion | 0.007s | 9.4× faster | same |
| 100 Billion | 0.034s | 21.2× faster | 3% |
| 1 Trillion | 0.168s | **51.4× faster** | 4.5% |
| 10 Trillion | 1.190s | **106.8× faster** | 3.3% |
