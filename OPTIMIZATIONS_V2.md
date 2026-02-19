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

## Optimization #1: Harmonic Block Technique (Branch 2)

For the "else branch" (j*p > √N), consecutive j values often produce the same
⌊(N/p)/j⌋. Instead of computing a division per j, we group j values into blocks
sharing the same quotient q, then subtract a single delta across the block.

Each block: 2 divisions + simple subtraction loop. Reduces total divisions from
O(N^{3/4}) to O(N^{1/2} × prime_harmonic_sum).

| Range | Before | After | Speedup |
|---|---|---|---|
| 100 Billion | 0.142s | 0.108s | 24% |
| 1 Trillion | 0.729s | 0.553s | **24%** |

---

## Optimization #2: i32 Small Array

Values in small[] are ≤ √N (at most ~3.16M for N=10T), which fits in i32.
Halves memory footprint of small[] from 8MB to 4MB, improving cache utilization.

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Trillion | 0.553s | ~0.55s | Marginal |

---

## Optimization #3: Barrett Fast Division (Small Update)

The inner loop `j / p` in the small[] update uses integer division (~21 cycles on x86).
Replaced with Barrett reduction: `recip_p = ceil(2^40 / p)`, then `j / p = (j * recip_p) >> 40`.
Exact for all j,p < 2^20 because j*(p-1) < 2^40.

| Range | Before | After | Speedup |
|---|---|---|---|
| 100 Billion | 0.108s | 0.101s | 7% |
| 1 Trillion | 0.553s | 0.492s | **11%** |

---

## Optimization #4: Unsafe Indexing

Replaced all array indexing in hot loops with `get_unchecked` / `get_unchecked_mut`
to eliminate bounds checks. The indices are mathematically guaranteed in-bounds.

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Trillion | 0.492s | 0.484s | 1.6% |

---

## Optimization #5: Primal Sieve for Prime Iteration

Replaced manual primality checks (`small[p] <= small[p-1]`) with `primal::Sieve`
to iterate only over primes in the outer loop. Eliminates composite-skipping branches.

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Trillion | 0.484s | 0.469s | 3% |
| 10 Trillion | 2.53s | 2.52s | ~0.5% |

---

## Current Best (V2)

| Range | Time | vs Sieve V1 (24 threads) |
|---|---|---|
| 1 Billion | 0.005s | 1.2× faster |
| 10 Billion | 0.022s | 3.0× faster |
| 100 Billion | 0.104s | 6.9× faster |
| 1 Trillion | 0.469s | **18.3× faster** |
| 10 Trillion | 2.52s | N/A (V1 untested) |
