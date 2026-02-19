# Optimization Log — V3 (Meissel-Lehmer Prime Counting)

Algorithm: Meissel-Lehmer extension of Lucy_Hedgehog.
Sieve primes only up to N^{1/3}, compute remaining P₂ contribution analytically.
Complexity: O(N^{2/3}) time, O(√N) space.

Hardware: **Intel Core Ultra 9 285K** (Arrow Lake, 8P+16E, 24 threads).
All times best-of-5, release mode, `lto=fat`, `codegen-units=1`, `target-cpu=native`.

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

The improvement is modest because the Phase 1 sieve (small update for 168 primes
with p ≤ √v) dominates runtime. The P₂ sum is essentially free (~77K lookups at 1T).

---

## Current Best (V3)

| Range | Time | vs Sieve V1 (24 threads) | vs V2 |
|---|---|---|---|
| 1 Billion | 0.002s | 3.0× faster | same |
| 10 Billion | 0.007s | 9.4× faster | same |
| 100 Billion | 0.034s | 21.2× faster | 3% |
| 1 Trillion | 0.170s | **50.8× faster** | 3.4% |
| 10 Trillion | 1.190s | **106.8× faster** | 3.3% |
