# Optimization Log — V4 (LMO Prime Counting)

Algorithm: Lagarias-Miller-Odlyzko (LMO).
Formula: π(x) = S1 + S2 + π(y) - 1 - P2, with y = x^{1/3} · α, a = π(y).
Complexity: O(N^{2/3} / log N) time, O(N^{1/3} · log²N) space.

Hardware: **Intel Core Ultra 9 285K** (Arrow Lake, 8P+16E, 24 threads).
All times best-of-5+, release mode, `lto=fat`, `codegen-units=1`, `target-cpu=native`.

References:
- Lagarias, Miller, Odlyzko: "Computing π(x): The Meissel-Lehmer Method" (1985)
- Kim Walisch: [primecount](https://github.com/kimwalisch/primecount) (pi_lmo_parallel.cpp)

---

## Phase 1: Recursive Phi (Correctness Baseline)

**Implementation**: Direct recursive φ(x,a) with memoization for S2 computation.
S1 uses PhiTiny cache (wheel-based O(1) for c ≤ 6).

**Bug fix**: `primal::Sieve::new(n).primes_from(2)` can return primes > n due to
internal block rounding. Fixed with `.take_while(|&p| p <= y)`. This caused 1M to
compute 78474 instead of 78498.

**Result** (α = 1.0):

| Range      | Time     | Status |
|------------|----------|--------|
| 1 Billion  | 0.069s   | ✓      |
| 10 Billion | 0.431s   | ✓      |
| 100 Billion| 3.464s   | ✓      |
| 1 Trillion | 22.856s  | ✓      |
| 10 Trillion| 151.804s | ✓      |

Correct but very slow — recursive phi is O(x^{2/3}) per call.

---

## Phase 2: Segmented Sieve S2

**Implementation**: Replaced recursive φ with segmented sieve + POPCNT counting.
Bit-packed sieve over [0, z] where z = x/y. For each segment:
1. Pre-sieve: cross off multiples of first c primes
2. For each prime index b: find special leaves, accumulate φ, cross off p_b
3. Hard leaves (b ≤ π(√y)): squarefree m with μ(m) ≠ 0, lpf(m) > p_b
4. Easy leaves (b > π(√y)): two-prime products

**Bug fix**: Position 0 in first segment represents integer 0, which must be
excluded from φ counts. Without `sieve.cross_off(0)` when low=0, every leaf
gets +1 overcounting from the accumulated phi[b].

**Result** (α = 1.0):

| Range      | Time    | vs Recursive |
|------------|---------|-------------|
| 1 Trillion | 0.366s  | **62× faster** |
| 10 Trillion| 2.387s  | **64× faster** |

---

## Optimization 1: Eliminate Remaining-b Loop ✅

**Hypothesis**: The "remaining b values" loop processes ALL b > max_b for every
segment, crossing off primes and accumulating phi[b] even when no leaves exist.
Since max_b decreases monotonically across segments, these b values will NEVER
have leaves in any future segment, so tracking them is wasted work.

**Change**: Remove the loop entirely. Only process b ∈ [c+1, max_b] per segment.

**Result** (α = 1.0):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 1 Trillion | 0.366s  | 0.168s  | **2.2×** |
| 10 Trillion| 2.387s  | 0.781s  | **3.1×** |

S2 went from 325ms → 130ms at 1T. The savings scale with x because later
segments (which dominate count) skip more b values.

---

## Optimization 2: Alpha Tuning ✅

**Hypothesis**: Increasing y = x^{1/3} · α shifts work from the expensive S2
(segmented sieve) to the cheaper P2 (primal sieve lookups) and S1 (PhiTiny O(1)).

**Results** (all correct):

| Alpha | 1T Time | 10T Time |
|-------|---------|----------|
| 1.0   | 0.168s  | 0.781s   |
| 2.0   | 0.127s  | 0.580s   |
| 3.0   | 0.119s  | 0.589s   |

α = 2.0 is optimal across all ranges. At 1T: S2 drops from 130ms → 86ms,
P2 stays ~35ms. The S2 reduction outweighs the P2 increase.

**Adopted**: α = 2.0

---

## Failed: Counter Array for O(1) Count ✗

**Hypothesis**: Replace linear-scan POPCNT count(pos) with prefix-sum counter
array for O(COUNTER_DIST) queries.

**Result**: 1T went from 0.127s → 0.193s (52% SLOWER). The counter update in
cross_off (O(num_blocks) per cross-off) dominated. Each cross_off must decrement
all subsequent counter entries.

**Root cause**: For small segment sizes (~256 bits = 4 words), the linear scan
costs only ~2 POPCNT operations. The counter overhead far exceeds the savings.
A batch "cross_off_count" approach (like primecount) could work but requires
more complex code.

**Reverted**.

---

## Optimization 3: Parallel P2 via Rayon ✅

**Hypothesis**: P2 computes π(x/p) for each prime p in (y, √x]. These lookups are
independent and embarrassingly parallel. The primal sieve is immutable and thread-safe.

**Change**: Collect primes into a Vec, then use `par_iter().map().sum()` via rayon.
Also removed per-component timing instrumentation to reduce overhead.

**Result** (α = 2.0):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 100 Billion| 0.032s  | 0.022s  | **1.5×** |
| 1 Trillion | 0.120s  | 0.098s  | **1.2×** |
| 10 Trillion| 0.560s  | 0.480s  | **1.2×** |

P2 was 28% of total time; parallelizing it across 24 threads nearly eliminates it.

---

## Failed: Tracked Total in BitSieve ✗

**Hypothesis**: Maintain a running `total` field in BitSieve, decrementing in cross_off,
to make count_total() O(1) instead of scanning all words.

**Result**: 1T went from 0.120s → 0.140s (17% SLOWER). The branch in cross_off
(checking if bit was set before decrementing) causes pipeline stalls. The branchless
`AND NOT` is faster than the conditional decrement.

**Reverted**.

---

## Failed: Segment Size Cap at 64K ✗

**Hypothesis**: Capping segment_size at 65536 bits (8KB) would improve L1 cache hits.

**Result**: No measurable difference — within noise of the uncapped version.
The default √z sizing already produces near-optimal segments.

**Reverted**.

---

## Current Best Performance (α = 2.0, parallel P2)

| Range       | V4 Time  | V3 Time  | Speedup vs V3 |
|-------------|----------|----------|----------------|
| 1 Billion   | 0.0015s  | 0.002s   | 1.3×           |
| 10 Billion  | 0.007s   | 0.007s   | 1.0×           |
| 100 Billion | 0.022s   | 0.034s   | 1.5×           |
| 1 Trillion  | 0.098s   | 0.168s   | **1.7×**       |
| 10 Trillion | 0.480s   | 1.190s   | **2.5×**       |

**Time breakdown at 1T** (pre-parallelization): tables 0.5ms, S1 0.04ms, S2 86ms (72%), P2 33ms (28%)

### Remaining Optimization Opportunities

1. **Parallel S2**: Split segments across rayon threads with phi_vector initialization
2. **Wheel-30 sieve**: Skip multiples of 2/3/5 in the sieve (8 bits per 30 numbers)
3. **Precompute valid m indices**: Skip composite m values in the inner loop
4. **Batch cross_off_count**: Combined sieve+counter update for large segments
