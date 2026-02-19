# Optimization Log — V2 (Combinatorial Prime Counting)

Algorithm: Lucy_Hedgehog / Meissel-Lehmer combinatorial method.
Computes π(N) in O(N^{3/4} / ln N) time, O(√N) space — no full sieve needed.

Hardware: **Intel Core Ultra 9 285K** (Arrow Lake, 8P+16E, 24 threads).
All times best-of-3, release mode, `lto=fat`, `codegen-units=1`, `target-cpu=native`.

---

## Baseline: Basic Lucy_Hedgehog

Single-threaded, no optimizations. Two arrays of size √N (small[] and large[]).
Iterates over each prime p ≤ √N, updating both arrays.

| Range | Time | vs Sieve V1 |
|---|---|---|
| 1 Billion | 0.0038s | 1.6× faster |
| 10 Billion | 0.027s | 2.4× faster |
| 100 Billion | 0.142s | 5.0× faster |
| 1 Trillion | 0.729s | **11.8× faster** |

The algorithm is inherently single-threaded (outer loop over primes is sequential),
yet it's already faster than the 24-thread sieve because it does O(N^{3/4}) work
instead of O(N).

---
