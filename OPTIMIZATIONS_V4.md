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

## Optimization 4: Incremental Count (count_delta) ✅

**Hypothesis**: In the special leaves inner loops, positions are monotonically increasing
(m decreases → xpm increases for hard leaves; q decreases → xpq increases for easy leaves).
Instead of scanning from position 0 for each count(), maintain a running count and only
scan the delta between consecutive positions.

**Change**: Added `count_delta(prev_pos, pos)` to BitSieve that counts set bits in
(prev_pos, pos] using partial word masks. Hard and easy leaf loops track running_count.

**Bug found and fixed**: `word >> (b0 + 1)` when b0=63 causes shift-by-64, which wraps
to shift-by-0 in release mode (Rust wrapping semantics), counting all 64 bits instead of 0.
Fixed with explicit mask: `if b0 < 63 { u64::MAX << (b0+1) } else { 0 }`.

**Result** (α = 2.0, with parallel P2):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 100 Billion| 0.022s  | 0.021s  | 1.0×    |
| 1 Trillion | 0.098s  | 0.091s  | **1.1×** |
| 10 Trillion| 0.480s  | 0.396s  | **1.2×** |

Savings increase with x because larger ranges have more leaves per segment,
amortizing more of the initial count() call across incremental deltas.

---

## Optimization 5: Branchless Cross-off + O(1) count_total ✅

**Two changes combined**:

1. **Branchless tracked total**: Extract bit value with `(old >> b) & 1` and subtract
   from total, making count_total() O(1) instead of O(nwords). Previous branchy version
   was slower, but this branchless version has no pipeline stalls.

2. **Remove dead `if low + k > 1` branch**: In all three cross-off loops, this check
   prevented crossing off position 1 (number 1). But no prime ≥ 2 ever has a multiple
   at position 1, so the check was dead code. Removing it eliminates one branch per
   cross-off iteration across ~millions of iterations.

**Failed**: Precomputed valid_m table — storing Vec<(m, mu)> per b was 2.5× SLOWER
due to precomputation cost, cache misses, and range-check overhead. Sequential access
to mu[] and lpf[] arrays is more cache-friendly.

**Result** (α = 2.0, with parallel P2 + incremental count):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 100 Billion| 0.021s  | 0.020s  | 1.1×    |
| 1 Trillion | 0.091s  | 0.084s  | **1.1×** |
| 10 Trillion| 0.396s  | 0.370s  | **1.1×** |

---

## Optimization 6: Re-tune Alpha to 2.5 ✅

**Hypothesis**: With P2 now parallelized, higher α values (larger y) are more
attractive because the increased P2 cost is absorbed by multiple threads.

**Results** (with all optimizations):

| Alpha | 1T Time | 10T Time |
|-------|---------|----------|
| 2.0   | 0.084s  | 0.370s   |
| 2.5   | 0.078s  | 0.340s   |
| 2.75  | 0.081s  | 0.338s   |
| 3.0   | 0.082s  | 0.343s   |
| 3.25  | 0.080s  | 0.340s   |
| 4.0   | 0.091s  | 0.388s   |

α = 2.5 is optimal for 1T; α = 2.5-3.0 is flat for 10T. Choosing α = 2.5.

**Adopted**: α = 2.5

---

## Optimization 7: Pre-sieve Template ✅ (MASSIVE WIN)

**Hypothesis**: The 6 pre-sieve cross-off loops (primes 2,3,5,7,11,13) perform
~21K individual cross_off() calls per 16K segment. Replace with a precomputed
template of period LCM(2,3,5,7,11,13) = 30,030 bits, stored doubled for wrapping.
Template is AND'd into the sieve in one pass over ~256 words.

**Change**: New `PreSieveTemplate` struct builds a 2×30030-bit pattern at init.
Each segment applies the template with a single word-aligned AND loop plus
POPCNT-based total adjustment. Replaces 6 separate cross-off loops entirely.

**Bug**: First version used single-period storage with manual wrapping — produced
small errors at period boundaries. Fixed by storing 2 full periods of template bits,
making the splice always access valid contiguous data.

**Result** (α = 2.5, with all prior optimizations):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 10 Billion | 0.005s  | 0.004s  | 1.3×    |
| 100 Billion| 0.019s  | 0.012s  | **1.6×** |
| 1 Trillion | 0.078s  | 0.042s  | **1.9×** |
| 10 Trillion| 0.340s  | 0.175s  | **1.9×** |

The pre-sieve was consuming ~50% of S2 time: 21K scalar cross_off operations
replaced by ~256 word-level AND + POPCNT operations (80× fewer operations).

---

## Optimization 8: Re-tune Alpha back to 2.0 ✅

**Hypothesis**: With the pre-sieve template eliminating pre-sieve cost, the balance
between S2 and P2 has shifted. Smaller y (lower alpha) means less P2 work.

**Results** (with pre-sieve template):

| Alpha | 1T Time | 10T Time |
|-------|---------|----------|
| 1.5   | 0.040s  | 0.169s   |
| 1.75  | 0.039s  | 0.164s   |
| 2.0   | 0.039s  | 0.162s   |
| 2.5   | 0.042s  | 0.175s   |

α = 2.0 is now optimal again (was 2.5 before pre-sieve template).

**Adopted**: α = 2.0

---

## Optimization 9: Concurrent S2 + P2 via thread::scope ✅

**Hypothesis**: S2 and P2 are independent computations. Running P2 sieve construction
and lookups in a background thread overlaps it entirely with S2.

**Change**: Use `std::thread::scope` to spawn P2 on a separate thread while S2
runs on the main thread. Both complete before the join.

**Result** (α = 2.0, with all prior optimizations):

| Range      | Before  | After   | Speedup |
|------------|---------|---------|---------|
| 100 Billion| 0.011s  | 0.008s  | **1.4×** |
| 1 Trillion | 0.040s  | 0.033s  | **1.2×** |
| 10 Trillion| 0.163s  | 0.137s  | **1.2×** |

P2 was ~20% of total time; overlapping it with S2 makes it essentially free.

**Failed**: Batched cross_off_step — building per-word masks for primes < 64 was
10% slower due to mask construction overhead and extra branches.

---

## Current Best Performance

| Range       | V4 Time  | V3 Time  | Speedup vs V3 |
|-------------|----------|----------|----------------|
| 1 Billion   | 0.0014s  | 0.002s   | 1.4×           |
| 10 Billion  | 0.003s   | 0.007s   | 2.3×           |
| 100 Billion | 0.008s   | 0.034s   | **4.3×**       |
| 1 Trillion  | 0.033s   | 0.168s   | **5.1×**       |
| 10 Trillion | 0.137s   | 1.190s   | **8.7×**       |

### Remaining Optimization Opportunities

1. **Parallel S2 segments**: Split segment range across threads (complex phi precomputation)
2. **Reduce integer divisions**: Reciprocal multiplication for x/(prime*m)
