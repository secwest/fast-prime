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

## Optimization 10: Combined init_sieve ✅

**Hypothesis**: Merging reset() + template.apply() + cross_off(0) into a single init_sieve() 
pass eliminates one full traversal of sieve words.

**Change**: New `init_sieve()` method on PreSieveTemplate copies template words directly
into sieve (computing popcount total on the fly), handles excess bits and position-0 
clearing in one pass. Removed dead `reset()` method.

**Result**: Marginal improvement (~2%), primarily visible at smaller scales.

---

## Optimization 11: Larger Segment Size (128K bits) ✅

**Hypothesis**: Default segment size √z is only 16K-32K at 10T scale. Larger segments
reduce per-segment overhead (init_sieve calls, prime start calculations, phi bookkeeping).

**Tested**: Minimum segment sizes 128K (16KB), 256K (32KB):
- 128K bits (16KB): fits L1 cache (48KB), ~5% faster at all scales
- 256K bits (32KB): L1 pressure causes regression

**Result** (128K minimum, α = 2.0):

| Range       | Before   | After    | Speedup |
|-------------|----------|----------|---------|
| 100 Billion | 0.009s   | 0.007s   | **1.3×** |
| 1 Trillion  | 0.034s   | 0.031s   | **1.1×** |
| 10 Trillion | 0.142s   | 0.134s   | **1.06×**|

---

## Optimization 12: Precompute x/prime ✅

**Hypothesis**: In inner loops, `x / (prime * m)` does a 128-bit multiply followed by 
division. Since prime is fixed within the inner loop, precomputing `x_div_prime = x / prime`
once and using `x_div_prime / m` eliminates one division.

**Math**: `floor(x / (a*b)) = floor(floor(x/a) / b)` — exact for integers.

**Result**: 1T 0.031s → 0.029s, 10T 0.134s → 0.126s (**6% faster**)

---

## Optimization 13: 4× Unrolled Cross-off Loop ✅

**Hypothesis**: Cross-off loop (`while k < seg_len { cross_off(k); k += p; }`) spends
significant time on loop control (branch, increment, compare). Unrolling by 4 reduces
loop overhead by 75%.

**Profiling showed**: cross_off = 46% of S2, easy_leaf = 44%, hard_leaf = 8%, init = 2%.

**Result** (8× unroll was SLOWER due to code bloat/register pressure):

| Range       | Before   | After    | Speedup |
|-------------|----------|----------|---------|
| 1 Trillion  | 0.029s   | 0.027s   | **1.07×**|
| 10 Trillion | 0.126s   | 0.117s   | **1.08×**|

---

## Optimization 14: Easy Leaf Position Reuse + Barrett Fast Division ✅

**Hypothesis**: Easy leaves dominate (44% of S2 = 57ms at 10T). Two optimizations:

1. When consecutive primes map to the same sieve position, skip the count query
   entirely and reuse the previous count (saves sieve access).

2. Barrett reciprocal table for primes: `recip[l] = floor(2^64 / primes[l])`. Only
   ~4600 entries = 37KB (fits L1 cache). Replaces 25-cycle hardware division with
   ~12-cycle branchless multiply-high + correction: `q = mulhi(n, recip) + (n - q*d >= d)`.

**Key insight**: Full reciprocal table for ALL m values (344KB) caused L2 cache pressure.
Primes-only table (37KB) fits in L1 with no pressure.

**Failed**: Barrett for ALL m values (344KB table → 4% SLOWER due to cache pressure).
**Failed**: Wheel mod 30 for hard leaf m iteration (overhead ≈ savings). 
**Failed**: Alpha retuning (1.5–3.0 tested, α=2.0 still optimal).

**Result**:

| Range       | Before   | After    | Speedup |
|-------------|----------|----------|---------|
| 100 Billion | 0.007s   | 0.007s   | ~same   |
| 1 Trillion  | 0.027s   | 0.027s   | ~same   |
| 10 Trillion | 0.117s   | 0.110s   | **1.06×**|

---

## Optimization 15: Inline Cross-off with Deferred Total ✅

**Hypothesis**: The `sieve.total -= was_set` in each cross_off creates a sequential 
dependency chain. Manually inlining the cross-off with 4× unrolling and accumulating 
delta in a local variable breaks this chain, allowing the CPU to parallelize independent 
word updates.

**Also tried and FAILED**: Word-level batching (build per-word mask for small primes p<64, 
apply via AND+XOR+POPCNT) — 10% SLOWER. Inner loop overhead (`while (k>>6)==w`) and 
XOR+POPCNT per word exceeded store-forwarding savings.

**Result**:

| Range       | Before   | After    | Speedup |
|-------------|----------|----------|---------|
| 1 Trillion  | 0.027s   | 0.026s   | ~same   |
| 10 Trillion | 0.110s   | 0.109s   | ~same   |

Marginal improvement from breaking the total dependency chain.

---

## Optimization 16: Parallel S2 via Delta-Phi Correction ✅ (MASSIVE WIN)

**Hypothesis**: S2 is the dominant bottleneck (75ms of 110ms at 10T). The segment loop
is sequential because `phi[b]` accumulates across segments. By splitting segments across
threads and correcting the phi offsets afterwards, we can parallelize S2.

**Technique**: Each thread processes a contiguous chunk of segments with local `phi[b]`
starting at 0. Additionally, each thread tracks a "coefficient" per prime b:
- For hard leaves: `coeff[b] -= mu[m]` for each leaf
- For easy leaves: `coeff[b] += 1` for each leaf

After all threads finish, the true phi at each thread's start is the prefix sum of
phi contributions from earlier threads. The correction for thread k is:
`Σ_b prefix_phi[b] × coeff[b]`

This is exact — no approximation. O(num_primes × num_threads) correction work.

**Implementation details**:
- Cross-off starting positions computed from scratch per segment (no `next[b]` state)
- Serial fallback for small inputs (≤2 segments)
- All rayon threads utilized for both S2 and P2 concurrently

**Result**:

| Range       | Before   | After    | Speedup |
|-------------|----------|----------|---------|
| 100 Billion | 0.007s   | 0.004s   | **1.8×** |
| 1 Trillion  | 0.026s   | 0.015s   | **1.7×** |
| 10 Trillion | 0.109s   | 0.062s   | **1.8×** |

---

## Optimization 17: Chunk Count Tuning + Truncated Correction ✅

**Hypothesis**: Early segments do 100× more work (max_b=4600) than late segments
(max_b=46). Using more chunks than threads enables rayon's work-stealing to balance
the load. Truncating the correction pass to max_b_seen (instead of all primes)
enables finer granularity without excessive overhead.

**Sweep results** (10T):

| Chunks      | Time    |
|-------------|---------|
| nthreads×4  | 0.055s  |
| nthreads×8  | 0.051s  |
| nthreads×10 | 0.050s  |
| nthreads×16 | 0.053s  |
| per-segment | 0.080s  |

**Chosen**: nthreads×8 with truncated correction.

**Result**: 10T 0.062s → 0.050s (~19% improvement)

---

## Optimization 18: Alpha Re-tune to 1.6 ✅

**Hypothesis**: With parallel S2, the S2 vs P2 balance has shifted. Lower alpha gives
more segments (more parallelism) and fewer primes (less per-segment work). P2 cost
increases but is overlapped with S2.

**Sweep results** (10T, 3 runs each):

| Alpha | Avg Time |
|-------|----------|
| 1.0   | blocked  |
| 1.2   | 0.052s   |
| 1.4   | 0.044s   |
| 1.5   | 0.042s   |
| 1.6   | 0.041s   |
| 1.8   | 0.044s   |
| 2.0   | 0.050s   |
| 2.5   | 0.066s   |

**Adopted**: α = 1.6

**Result**: 10T 0.050s → 0.042s (~16% improvement)

---

## Optimization 19: mimalloc Global Allocator ✅

**Insight**: Profiling revealed that per-chunk Vec allocation (768 chunks × 74KB each = 57MB)
consumed nearly as much time as actual computation (129ms allocation vs 147ms work). Windows'
default heap allocator is slow under multi-threaded contention.

**Change**: Added `mimalloc` as global allocator. mimalloc is designed for high-throughput
multi-threaded allocation with thread-local heaps that minimize lock contention.

**Result**: Allocation overhead dropped from 129ms to ~15ms (8.6× faster allocation).
Overall S2 parallel time improved from ~42ms to ~35-40ms.

| Range       | Before  | After   | Change |
|-------------|---------|---------|--------|
| 1 Trillion  | 0.010s  | 0.010s  | same   |
| 10 Trillion | 0.042s  | 0.040s  | -5%    |

## Optimization 20: Pi-Formula for Segment 0 Easy Leaves ✅

**Insight**: Profiling showed segment 0 has 4.7M easy leaf iterations taking 19.5ms.
For these leaves, when primes[b-1]² ≥ high, the identity phi(n, b-1) = 1 + max(pi(n) - (b-1), 0)
allows replacing sieve counting with a simple pi table lookup.

**Change**: Build a small pi lookup table covering [0, segment_size] (131KB, negligible cost).
For qualifying easy leaves in segment 0:
- Leaves where b-1 > pi(xpq): phi=1, batch-counted in O(1) per prime (no Barrett division)
- Remaining leaves: use pi_table[xpq] lookup instead of sieve.count()/count_delta()

This eliminates all sieve scanning (572-word count() per prime, 5-cycle count_delta() per leaf)
for segment 0's easy leaves. The batch-counting path also eliminates Barrett divisions for the
majority of leaves.

**Result**: 10T improved from 0.040s to 0.036s (~10% improvement).

| Range       | Before  | After   | Change |
|-------------|---------|---------|--------|
| 1 Trillion  | 0.010s  | 0.008s  | -20%   |
| 10 Trillion | 0.040s  | 0.036s  | -10%   |

## Optimization 21: Alpha Re-tune to 2.2 ✅

**Hypothesis**: With the pi-formula optimization reducing per-leaf cost in segment 0,
the optimal alpha may have shifted. Higher alpha means larger y, more primes, but the
pi-formula handles the increased easy leaf load efficiently.

**Sweep results** (10T, 8 runs each, showing best):

| Alpha | Best    | Median  |
|-------|---------|---------|
| 1.6   | 0.035s  | 0.038s  |
| 1.9   | 0.031s  | 0.034s  |
| 2.1   | 0.031s  | 0.033s  |
| 2.2   | 0.029s  | 0.031s  |
| 2.3   | 0.031s  | 0.033s  |
| 2.4   | 0.031s  | 0.032s  |

**Adopted**: α = 2.2

**Result**: 10T 0.036s → 0.031s (~14% improvement)

## Optimization 22: Adaptive Segment Size ✅

**Hypothesis**: Larger segments reduce per-segment overhead (fewer template inits, fewer
correction entries), but too-large segments reduce parallelism for smaller inputs. An
adaptive formula scales with problem size.

**Formula**: `segment_size = max(z / (num_threads * 32), 2^17).next_power_of_two()`

This gives: 2^17 (131K) for inputs up to ~1T, 2^19 (524K) for 10T. Ensures at least
768 segments for parallelism while using larger segments when z is large enough.

**Sweep showed** (10T): 2^17=0.031s, 2^18=0.029s, 2^19=0.028s, 2^20=0.029s, 2^21=0.034s.

**Result**: 10T improved from fixed 2^17 (0.031s) to adaptive (0.029s), without regressing
smaller ranges.

| Range       | Before  | After   | Change |
|-------------|---------|---------|--------|
| 100 Billion | 0.003s  | 0.003s  | same   |
| 1 Trillion  | 0.007s  | 0.007s  | same   |
| 10 Trillion | 0.031s  | 0.029s  | -6%    |

## Optimization 23: Reduce Chunk Count to 6× Threads ✅

**Insight**: Fewer chunks means fewer entries in the correction pass (sequential O(nchunks × max_b)
dot-product and prefix-sum). With work-stealing, rayon still achieves good load balance even with
fewer chunks. More chunks add correction overhead without improving peak performance.

**Sweep** (10T, 8+ runs):

| Multiplier | Chunks | Best   | Median |
|------------|--------|--------|--------|
| 4×         | 96     | 0.028s | 0.028s |
| 6×         | 144    | 0.027s | 0.029s |
| 8×         | 192    | 0.027s | 0.029s |
| 16×        | 384    | 0.028s | 0.030s |
| 32×        | 768    | 0.029s | 0.029s |

**Adopted**: 6× (good balance of low overhead and reliable scheduling)

**Result**: 10T 0.029s → 0.028s (best), correction pass ~3× faster.

## Optimization 24: p³ Full Batch for Large Easy Primes ✅

When `primes[b]³ ≥ x`, ALL easy leaves of prime b have `phi = 1` (because
`xpq < primes[b-1]` for all q). This lets us count all leaves in O(1) per prime
without threshold computation or individual pi lookups.

**Result**: Marginal improvement (eliminates ~2500 individual lookups at 10T).

## Optimization 25: Parallel P2 Sieve ✅ (MAJOR WIN)

**Profiling discovery**: P2 was the **actual bottleneck**, not S2!
- At 10T: P2 took 27ms (single-threaded `primal::Sieve::new(211M)`)
- S2 par_iter took only 17ms
- `thread::scope(max(S2, P2))` = 27ms, dominated by P2

**Solution**: Replace `primal::Sieve` with custom `ParallelPiSieve`:
- Odd-number bitmap (bit i = is_prime(2i+1)), parallel sieve via `par_chunks_mut`
- Prefix-sum array for O(1) π(n) queries using popcount
- Runs concurrently with S2 via `thread::scope`, sharing rayon's thread pool

**Why it works**: primal's `Sieve::new()` is completely single-threaded. Our
`ParallelPiSieve` distributes the cross-off work across all available threads.
Even when competing with S2 for rayon threads, the work-stealing scheduler
naturally balances the load.

**P2 standalone**: 27ms → 13ms (with full rayon access)
**P2 concurrent with S2**: ~20ms (sharing threads), but total improves because
both finish faster than the old primal bottleneck.

**Result**:

| Range       | Before  | After   | Speedup |
|-------------|---------|---------|---------|
| 1 Trillion  | 0.007s  | 0.006s  | **17%** |
| 10 Trillion | 0.028s  | 0.022s  | **21%** |

## Failed: Profile-Guided Optimization (PGO) ✗

**Hypothesis**: Rust/LLVM PGO uses runtime branch profile data to optimize code layout,
branch prediction hints, and inlining decisions. Typically yields 5-15% on complex code.

**Change**: Built with `-Cprofile-generate`, ran full benchmark suite for training data,
merged profiles with `llvm-profdata`, rebuilt with `-Cprofile-use`.

**Result**: No improvement — within noise at all scales. LTO=fat + codegen-units=1 already
captures most of the benefits that PGO provides. The hot loops are manually unrolled with
minimal branching, leaving little for PGO to improve.

**Reverted** (no code changes were needed).

---

## Optimization 27: Adaptive Alpha Scaling ✅ (MAJOR WIN for large inputs)

**Discovery**: Alpha sweep revealed dramatically different optima per input size:

| Alpha | 10T best | 100T best | 1Q best  |
|-------|----------|-----------|----------|
| 2.2   | 0.022s   | 0.103s    | 2.21s    |
| 2.4   | 0.026s   | 0.099s    | ~1.95s   |
| 3.0   | 0.027s   | 0.110s    | 1.58s    |
| 4.0   | 0.031s   | 0.128s    | 1.06s    |
| 5.0   | 0.037s   | 0.133s    | 0.83s    |
| 6.0   | 0.042s   | 0.170s    | 0.79s    |
| 8.0   | ~0.05s   | 0.191s    | 0.94s    |

**Root cause**: For larger x, segments have more primes and longer inner loops,
so the per-prime overhead (starting position calculation, phi tracking) is better
amortized. Higher alpha means larger y and fewer segments (smaller z = x/y), which
reduces total S2 work. At 1Q, even α=6.0 gives 3200+ segments — plenty for parallelism.

**Formula**: `alpha = f(log10(x))`
- x ≤ 10^13: α = 2.2
- 10^13 < x ≤ 10^14: α = 2.2 + 0.2·(log₁₀x - 13) → 2.2 to 2.4
- x > 10^14: α = 2.4 + 3.6·(log₁₀x - 14) → 6.0 at 1Q

**Result**:

| Range         | Before  | After   | Speedup |
|---------------|---------|---------|---------|
| 1 Trillion    | 0.006s  | 0.006s  | same    |
| 10 Trillion   | 0.022s  | 0.022s  | same    |
| 100 Trillion  | 0.103s  | 0.099s  | **4%**  |
| 1 Quadrillion | 2.210s  | 0.793s  | **64%** |

---

## Failed: P2 Pre-sieve Template ✗

**Hypothesis**: Apply a 15015-period template (primes 3,5,7,11,13) to the P2 odd-number
bitmap, skipping ~77% of small-prime cross-offs.

**Result**: Wash. Template tiling overhead (get_word per word) cancels the cross-off
savings. Sequential tiling was even worse due to memory bandwidth for large sieves (1.67B
at 1Q). Parallel tiling per chunk was break-even.

**Reverted**.

---

## Failed: Dedicated Rayon Thread Pools ✗

**Hypothesis**: Give P2 and S2 separate rayon thread pools to eliminate work-stealing
contention.

**Result**: 40-85% WORSE. The shared global pool's work-stealing naturally balances load
between P2 and S2. Separate pools prevent this, and pool creation adds ~1ms overhead.

**Reverted**.

---

## Optimization 28: Skip Cross-off for Primes > √high ✅

**Insight**: When prime p > √(segment_high), every composite multiple of p in the
segment has already been cleared by a smaller prime. The only bit to clear is p itself
(if it's in the segment). This replaces O(segment_size/p) cross-off iterations with
a single O(1) bit check.

**Proof**: For composite c = k·p in [low, high), k ≥ 2. Since c < high, we have
c ≤ high-1. The smallest prime factor of c is ≤ √c ≤ √(high-1) < p. So this factor
has already been crossed off.

**Profiling** (1Q): At α=6.0, S2=797ms dominates. Components:
- S2 chunk 0 (segments 0-2): ~49K primes per segment
- Primes > √4M ≈ 2000: ~47K primes skip full cross-off loop
- Savings: ~22.8M iterations/segment avoided for large primes

**Result**: Marginal improvement at 100T scale (~3-7%); within noise at other scales.
Kept for architectural correctness.

---

## Optimization 29: Extended Alpha Tuning for 10Q/100Q ✅

Added 10 Quadrillion (10^16) and 100 Quadrillion (10^17) benchmark cases.

Alpha sweep for 10Q revealed the previous formula (α=9.6) was suboptimal:

| Alpha | 10Q Time |
|-------|----------|
| 6     | 7.36s    |
| 8     | 6.50s    |
| 10    | 6.07s    |
| 12    | 5.94s    |
| **13**| **5.67s**|
| 14    | 5.78s    |
| 16    | 6.18s    |
| 20    | 6.49s    |

For 100Q, α=16 was optimal (34.7s), close to the previous formula (α=13.2 → 35.2s).

**Updated formula** (piecewise linear in log₁₀(x)):
- x ≤ 10^13: α = 2.2
- 10^13 < x ≤ 10^14: slope 0.2 → 2.4
- 10^14 < x ≤ 10^15: slope 3.6 → 6.0
- 10^15 < x ≤ 10^16: slope 7.0 → 13.0
- x > 10^16: slope 3.0 → 16.0 at 10^17

**Result**: 10Q improved 9% (6.25s → 5.68s). All other scales unchanged.

---

## Optimization 30: Odd-only S2 Sieve ✅

**Insight**: The S2 sieve represents ALL integers in each segment, but all even positions
are always zero (crossed off by the template since prime 2 is in the template). Half the
sieve memory is wasted on guaranteed-zero bits.

**Changes**:
- Sieve bit i now represents odd integer `low + 2*i + 1` (half the memory)
- Template period reduced from lcm(2,3,5,7,11,13)=30030 to lcm(3,5,7,11,13)=15015
- Cross-off only visits odd multiples: step = p in bit space (since odd multiples of p
  are 2p apart, and each bit covers 2 integers)
- `int_to_odd_bp(n, low) = (n - low - 1) / 2` maps integer to bit position
- `first_odd_multiple(p, bound)` finds starting position for cross-off

**Benefits**:
- Sieve memory halved: 512KB → 256KB per segment (closer to L2)
- Cross-off iterations halved: only odd multiples visited
- Template init halved: fewer words to tile
- count/count_delta faster: fewer bits to scan

**Result**:

| Range          | Before  | After   | Improvement |
|----------------|---------|---------|-------------|
| 1 Trillion     | 0.006s  | 0.005s  | 17%         |
| 10 Trillion    | 0.022s  | 0.019s  | **14%**     |
| 100 Trillion   | 0.096s  | 0.091s  | **5%**      |
| 1 Quadrillion  | 0.793s  | 0.755s  | **5%**      |
| 10 Quadrillion | 5.680s  | 5.430s  | **4%**      |
| 100 Quadrillion| 34.33s  | 33.63s  | **2%**      |

---

## Optimization 31: P2 Prefix u32→u64 + Max i64 Benchmark ✅

**Problem**: `ParallelPiSieve` stored prefix counts as `Vec<u32>`. At large inputs where
π(z) > 4.3B (z > ~115B), the prefix overflows u32 max. This blocks computing π(2^63-1)
where z ≈ 200B and π(z) ≈ 7.7B.

**Fix**: Changed `prefix: Vec<u32>` → `Vec<u64>` in struct, construction, and accumulation.
Memory doubles for prefix table (12.5GB at 2^63-1 scale), but 96GB RAM is sufficient.

**Result**: π(2^63-1) = π(9,223,372,036,854,775,807) = **216,289,611,853,439,384** primes,
computed in **939.2 seconds** (~15.65 minutes). Consistent with Li(2^63-1) ≈ 2.17 × 10^17.
All existing benchmarks verified correct (no regression).

---

## Current Best Performance

| Range          | V4 Time  | V3 Time  | Speedup vs V3 |
|----------------|----------|----------|----------------|
| 1 Billion      | 0.0009s  | 0.002s   | 2.2×           |
| 10 Billion     | 0.002s   | 0.007s   | 3.5×           |
| 100 Billion    | 0.003s   | 0.034s   | **11.3×**      |
| 1 Trillion     | 0.005s   | 0.168s   | **33.6×**      |
| 10 Trillion    | 0.019s   | 1.190s   | **62.6×**      |
| 100 Trillion   | 0.091s   |    —     |       —        |
| 1 Quadrillion  | 0.755s   |    —     |       —        |
| 10 Quadrillion | 5.430s   |    —     |       —        |
| 100 Quadrillion| 33.63s   |    —     |       —        |
| 1 Quintillion  | 192.0s   |    —     |       —        |
| Max i64 (2⁶³-1)| 939.2s  |    —     |       —        |

### Remaining Optimization Opportunities

1. **Extend pi-formula to more segments**: Build per-segment pi tables from frozen sieve state
2. **Gourdon's algorithm**: O(x^{2/3} / log²x), fundamentally better complexity
3. **SIMD correction pass**: AVX2 dot-product for the sequential correction loop
4. **Bucket sieve**: Maintain prime-to-segment mapping to avoid per-segment startup cost

---

## Failed: Wheel-30 S2 Sieve ✗

**Hypothesis**: Replace odd-only sieve (1 bit per 2 integers) with wheel-30 sieve (8 bits per
30 integers). Only represents numbers coprime to {2,3,5}, reducing sieve memory and cross-off
by ~47% (8/30 vs 1/2 density).

**Implementation**: Full wheel-30 with precomputed step tables (`compute_wheel_steps`), wheel
position mapping (`int_to_wheel_bp`), and modified template (period=8008 bits for primes
{7,11,13}). Segment size rounded to multiple of 30. All correctness tests pass.

**Results**: 3-21% SLOWER across all scales.

| Range          | Odd-only | Wheel-30 | Change    |
|----------------|----------|----------|-----------|
| 10 Trillion    | 0.019s   | 0.023s   | +21% ✗   |
| 100 Trillion   | 0.091s   | 0.109s   | +20% ✗   |
| 1 Quadrillion  | 0.755s   | 0.776s   | +3% ✗    |
| 10 Quadrillion | 5.430s   | 5.683s   | +5% ✗    |
| 100 Quadrillion| 33.63s   | 34.90s   | +4% ✗    |

**Why it failed**:
1. **Variable stride defeats prefetcher** — Odd-only uses constant step `k += p` which the
   hardware prefetcher learns instantly. Wheel-30 uses `k += steps[ci]` with 8 different
   step sizes, breaking stride prediction.
2. **Expensive position mapping** — `int_to_wheel_bp` requires division/modulo by 30 (compiled
   to multiply+shift, ~5 instructions). Odd-only uses shift-by-1 (~1 instruction).
3. **Loss of 4× unrolling** — The 4× unrolled odd-only loop cannot be trivially adapted to
   variable-stride wheel-30 stepping.
4. **Diminishing returns at scale** — Degradation is worst at smaller scales (21% at 10T)
   where per-iteration overhead dominates. At larger scales (4% at 100Q) the iteration
   reduction starts to compensate, but never enough to break even.

---

## Failed: Batch Counting Extension to All Segments ✗

**Hypothesis**: The pi-formula batch counting (p³ ≥ x → all leaves have φ=1, counted in O(1))
is currently only applied on segment 0. Extending it to all segments should reduce easy leaf
iterations.

**Result**: 2-79% SLOWER. The p³ ≥ x condition rarely triggers at scales < 10Q (requires
p > x^{1/3} but primes only go up to y ≈ x^{1/3}·α). The added condition check
(u128 multiply + comparison) runs for every prime on every segment with zero benefit.

---

## Failed: Segment Size Cap (L2 Cache Fit) ✗

**Hypothesis**: At large scales (10Q+), segment_size grows to ~64M (odd-only sieve = 4MB),
exceeding L2 cache (2MB per P-core). Capping segment_size to fit L2 should improve cross-off
cache locality.

**Results**: Caps at 2^22 (sieve=256KB) and 2^24 (sieve=1MB) both tested.
- 2^22 cap: 100Q 5% faster, but 1Q 2%, 10Q 11%, 10T 68% SLOWER.
- 2^24 cap: 1Q 34% SLOWER, everything else worse or neutral.
- The increased segment count adds overhead (template init, phi updates, correction pass)
  that overwhelms any cache benefit.

---

## Failed: Chunk Count Tuning ✗

**Hypothesis**: The parallel S2 uses `nchunks = threads × 6`. More chunks (× 16) could improve
work-stealing for skewed workloads. Fewer chunks (× 4) could reduce correction pass overhead.

**Result**: Both directions worse. × 16 gives 9-89% regression (correction pass overhead
dominates). × 4 gives 9-13% regression (insufficient work-stealing for skewed early segments).
The original × 6 multiplier is the optimal sweet spot.
