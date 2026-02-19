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

## Optimization #6: Two-Phase Harmonic Iteration

Split branch 2 at √(N/p): Phase A iterates j with singleton blocks (1 division per j),
Phase B iterates q downward with multi-element blocks (1 division per q, carry first_j
forward). Halves the total number of integer divisions.

| Range | Before | After | Speedup |
|---|---|---|---|
| 100 Billion | 0.104s | 0.055s | 47% |
| 1 Trillion | 0.469s | 0.275s | **41%** |
| 10 Trillion | 2.52s | 1.67s | **34%** |

---

## Optimization #7: p=2 Shift in Small Update

Special-case the p=2 iteration of the small[] reverse update: `j/2` becomes `j >> 1`
(single instruction vs Barrett multiply+shift).

| Range | Before | After | Speedup |
|---|---|---|---|
| 1 Trillion | 0.275s | 0.270s | 2% |

---

## Optimization #8: Reciprocal Table for Division Elimination

Precompute `recip[j] = ceil(2^64 / j)` for all j ≤ √N. Then `n_div_p / j` becomes
`(n_div_p as u128 * recip[j] as u128) >> 64` — a multiplication (~4 cycles) instead of
integer DIVQ (~25 cycles). Applied to both Phase A and Phase B inner loops.

Table cost: O(√N) one-time build (u128 divisions), 8MB memory.
Exact for all n_div_p < 2^64 (always true for our range).

Also added hybrid carry-forward in Phase B for primes p ≤ 7 (where the average
increment per q-step is ≤ 2, making multiply+compare cheaper than division).

| Range | Before | After | Speedup |
|---|---|---|---|
| 100 Billion | 0.055s | 0.048s | 13% |
| 1 Trillion | 0.270s | 0.232s | **14%** |
| 10 Trillion | 1.67s | 1.48s | **11%** |

---

## Optimization #9: Phase A/B 4× Unroll

Unroll the Phase A and Phase B loops by 4 to allow the CPU to pipeline
independent u128 reciprocal multiplies (MULQ has 3-cycle latency, 1-cycle
throughput — 4 independent multiplies keep the pipeline full).

| Range | Before | After | Speedup |
|---|---|---|---|
| 100 Billion | 0.048s | 0.043s | 10% |
| 1 Trillion | 0.232s | 0.212s | **8.6%** |
| 10 Trillion | 1.48s | 1.37s | **7.4%** |

---

## Failed Attempts

- **Skip delta==0 blocks in branch 2**: Extra branch misprediction outweighed savings.
- **Software prefetch in branch 1**: Hardware prefetcher already effective on Arrow Lake.
- **Pre-sieve p=2 in initialization**: Correct but .max(0) in init offset savings. Neutral.
- **Pointer-based branch 1**: LLVM already generates same code. No change.
- **f64 division in Phase A/B**: DIVSD similar throughput to DIVQ on Arrow Lake. No gain.
- **Magic number for p=3 small update**: Barrett already equivalent. No gain.
- **Carry-forward last_j for ALL Phase B primes**: Too many increments per step for larger primes. 14% slower.
- **Reciprocal table for setup divisions**: Only 2 divisions per prime; u128 overhead > savings.
- **Uniform reciprocal table (remove p≤7 hybrid)**: Slightly worse — carry-forward helps small primes.
- **Reverse Phase A iteration**: No cache benefit; hardware prefetcher already effective.

---

## Current Best (V2)

| Range | Time | vs Sieve V1 (24 threads) |
|---|---|---|
| 1 Billion | 0.002s | 3.0× faster |
| 10 Billion | 0.010s | 6.6× faster |
| 100 Billion | 0.043s | 16.8× faster |
| 1 Trillion | 0.212s | **40.8× faster** |
| 10 Trillion | 1.37s | **92.8× faster** |
