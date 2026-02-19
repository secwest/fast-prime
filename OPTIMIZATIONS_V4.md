# Optimization Log — V4 (LMO Prime Counting)

Algorithm: Lagarias-Miller-Odlyzko (LMO).
Formula: π(x) = S1 + S2 + π(y) - 1 - P2, with y = x^{1/3} · α, a = π(y).
Complexity: O(N^{2/3} / log N) time, O(N^{1/3} · log²N) space.

Hardware: **Intel Core Ultra 9 285K** (Arrow Lake, 8P+16E, 24 threads).
All times best-of-5+, release mode, `lto=fat`, `codegen-units=1`, `target-cpu=native`.

References:
- Lagarias, Miller, Odlyzko: "Computing π(x): The Meissel-Lehmer Method" (1985)
- Kim Walisch: [primecount](https://github.com/kimwalisch/primecount) (pi_lmo5.cpp)

---

## Baseline: Initial LMO Implementation

*Pending first benchmark...*
