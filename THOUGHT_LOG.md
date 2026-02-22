# V3 Optimization Thought Log

## Session: Continued V3 Optimization (Post-Profiling)

### Attempt 1: Parallel Initialization (FAILED — 3 sub-attempts)

**Idea**: Use `std::thread::scope` to overlap the three independent init tasks:
1. Reciprocal table build (u128 division, ~5ms at 1T)
2. small[] + large[] array init (~2ms at 1T)
3. Primal sieve construction

**Sub-attempt 1a**: 3 threads (recip + sieve + small/large)
- Result: 1T 0.173s vs 0.168s = 3% WORSE

**Sub-attempt 1b**: 2 threads (recip on background, main does small/large)
- Result: 1T 0.177s vs 0.168s = 5% WORSE

**Sub-attempt 1c**: Parallel P₂ with rayon par_chunks
- Adds Vec allocation + rayon overhead for <1ms computation

**Root cause**: OS thread spawn overhead (~50-100μs per thread) exceeds the 
overlap savings (~2ms at best). Init is only 3% of total time.

**Verdict**: REVERTED. Thread parallelism doesn't help when sequential work is fast.

---

### Attempt 2: Phase B Prime Batching (INCORRECT — REVERTED)

**Idea**: Since π(q) is constant between consecutive primes, skip composite q values
in Phase B and only compute u128 multiply boundaries at prime positions. Expected 
~40× reduction in u128 multiplies.

**Critical Error Discovered**: The intermediate small[] array during the sieve is 
NOT π(q)! After sieving primes 2,...,(p-1), small[q] counts integers ≤ q not 
divisible by any of those primes. "Surviving composites" (products of primes ≥ p) 
cause small[q] to change at NON-prime positions.

**Example**: For p=5 during sieve, small[25] changes because 25=5² survives the 
sieve of {2,3} — it has no prime factor < 5. Between primes 23 and 29:
- small[23] = 7, small[24] = 7 (24=2³×3, sieved out)
- small[25] = 8 ← COMPOSITE but surviving!
- small[26] = 8, ..., small[28] = 8 (all sieved out)
- small[29] = 9 (prime, survives)

**Result**: All counts too LOW (1M: 78482 vs 78498, off by 16). Reverted.

**Key Insight**: small[q] = π(q) ONLY after ALL sieve primes are processed. During 
the sieve, the "constant intervals" of small[] are bounded by surviving numbers 
(density ~20-40% of all positions), not just primes (~8% of positions).

---

### Attempt 3: Phase B Merged Constant-Delta Runs (CORRECT but SLOWER)

**Idea**: Scan small[] values to detect transitions (where small[q] changes), only 
computing u128 boundary multiply at transition points. Merge consecutive fills with 
the same delta into longer fills (better vectorization).

**Implementation**: Inner while loop scans backward through small[] checking if 
small[q-1] == small[q]. At each transition, compute lj via reciprocal multiply 
and do a merged fill.

**Result**: 
- 1T: 0.187s vs 0.168s baseline = **11% WORSE**
- 10T: 1.271s vs 0.190s baseline = **7% WORSE**

**Root cause**: Branch misprediction in the scanning inner loop.
- ~15M transitions in 65M q values = ~23% transition rate
- Branch misprediction cost: 15M × 14 cycles = 210M cycles ≈ 38ms
- This overwhelms the multiply savings (~150M cycles ≈ 27ms saved)
- Net: ~85M extra cycles of pure branch misprediction overhead

**Why the original 4× unrolled Phase B code wins**:
- u128 multiply: 3-cycle latency, 1-cycle throughput
- 4× unrolling saturates the multiply pipeline (4 independent ops in flight)
- NO branch misprediction — multiply is data-dependent but branchless
- Short fills (1-2 elements) have predictable loop bounds
- Achieves ~3.5 cycles/op, close to theoretical 3-cycle minimum

**Verdict**: REVERTED.

---

### Attempt 4: 8× Unrolling Phase A + Small Update (WORSE)
Doubled unroll factor from 4× to 8× for both Phase A and non-p=2 small update.
**Result**: 1T 0.189s vs 0.168s = **13% WORSE**. Instruction cache pressure from
larger loop body. CPU's OoO engine already extracts maximum ILP from 4×.

### Attempt 5: AVX-512 SIMD Investigation (NOT AVAILABLE)
Arrow Lake does NOT support AVX-512 — only AVX2. Detailed analysis:
- u128 multiply via Karatsuba vpmuludq: ~3 cycles/mul vs scalar mulxq at 1c/mul
- AVX2 gather: ~20 cycles for 4 elements vs ~4 cycles for 4 scalar loads
- AVXIFMA (52-bit multiply): available but gather bottleneck negates gains
- **No viable SIMD path** on this hardware

### Attempt 6: Profile-Guided Optimization (NEUTRAL)
Built with -Cprofile-generate, trained on full benchmark, rebuilt with -Cprofile-use.
**Result**: 1T avg 0.175s vs baseline 0.177s = within measurement noise.

---

## Conclusions on V3 Optimization

### All Optimization Avenues Exhausted

**Large update (81% of time)**:
- Phase A: u128 multiply throughput saturated (4× unrolled)
- Phase B: merge-by-transition adds branch misprediction > multiply savings
- Phase B: prime-batching incorrect (intermediate small[] ≠ π(q))
- Software prefetching: neutral (hardware prefetcher adequate)
- The 3.5 cycles/op is hardware-limited (u128 multiply + cache access)

**Small update (15%)**:
- Already uses 40-bit reciprocal multiply (no u128 needed)
- 4× unrolled, p=2 special-cased with shift
- Could try SIMD but the random small[j/p] access pattern prevents vectorization

**Init (3%)**:
- Parallel init adds more thread overhead than it saves
- f64 reciprocal approximation: incorrect (53-bit mantissa too imprecise)

**P₂ (<1%)**:
- Negligible, already optimized (on-demand iterator)

### What Would Help (but requires different algorithm)
1. **Deleglise-Rivat / LMO**: O(N^{2/3}/log(N)) vs current O(N^{2/3})
   - Reduces sieve prime count and avoids the bottleneck entirely
   - Complete rewrite required
2. **AVX-512 intrinsics**: Could scan small[] for transitions branchlessly (SIMD compare)
   - ~16 elements per cycle instead of 1 → eliminates misprediction overhead
   - But adds ~200 lines of platform-specific intrinsics code
3. **GPU offload**: The large update is embarrassingly data-parallel per j value
   - But data dependency across primes prevents simple parallelism

### Profiling Reference (1T)
- Init: 5.8ms (3.1%)
- Large update: 151.4ms (81.3%)
  - Branch 1: ~2.3M ops
  - Phase A: ~36M ops (u128 multiply per j)
  - Phase B unrolled: ~131M ops (u128 multiply per q)
  - Phase B carry-forward: ~1.9M ops
- Small update: 27.6ms (14.8%)
- P₂: <1ms

### Current Best V3 Performance
- 1B: 0.002s ✓
- 10B: 0.007s ✓
- 100B: 0.034s ✓
- 1T: 0.168s ✓ (51.4× vs V1 24-thread sieve)
- 10T: 1.190s ✓ (106.8× vs sieve)

---

## V4 LMO Implementation — Chain of Thought

### Algorithm Selection
After exhausting V3 optimization (15+ failed attempts, confirmed at hardware limit ~3.5 cycles/op), chose LMO for V4 because:
- O(N^{2/3} / log N) vs V3's O(N^{2/3}) — the /log N factor matters at 10T+
- Completely different algorithm — new optimization opportunities
- Reference: Kim Walisch's primecount (state of the art)

### Implementation Strategy
1. Start with pi_lmo1.cpp (simple recursive phi) for correctness verification
2. Switch to pi_lmo_parallel.cpp (segmented sieve) for performance
3. Optimize incrementally, committing each step

### Bug 1: primal Sieve boundary
primal::Sieve::new(100).primes_from(2) returned prime 101 due to internal block rounding.
This caused pi_y=26 instead of 25 for y=100 (x=1M). Fixed with .take_while(|&p| p <= y).
Error was -24 primes (78474 vs 78498).

### Bug 2: Sieve position 0
Position 0 in first segment represents integer 0, which is NOT in [1, x].
Without crossing it off, every phi[b] gets +1 overcounting from count_total().
The overcounting grows with x because every leaf in every segment gets the inflated phi.
- 100K: +28, 1M: +171, 1B: +12999, 10T: +2,892,628
Fixed with if low == 0 { sieve.cross_off(0); }.

### Key Insight: Remaining-b Loop
The "remaining b" loop in compute_s2 processes ALL b > max_b for every segment.
Since max_b = π[min(√(x/low), y)] DECREASES as low increases, any b > max_b
in the current segment will also exceed max_b in ALL future segments.
Therefore phi[b] and cross-off for these b values are never used — pure waste.
Removing this loop gave 2-3× speedup (1T: 0.366s → 0.168s).

### Counter Array Attempt (FAILED)
Tried prefix-sum counter array for O(COUNTER_DIST) count queries.
cross_off needs to update all subsequent counters = O(n_blocks) per cross_off.
For small segments (~256 bits), linear POPCNT scan costs ~2 ops while counter
overhead is ~19 decrements. Net result: 52% SLOWER. Need batch cross_off_count
approach (like primecount) to amortize updates.

### Alpha Tuning
α=2.0 shifts work from S2 (78% of time) to P2 (22%).
S2 savings: z = x/y = x/(2·x^{1/3}) = x^{2/3}/2 — half the segments.
P2 cost: more primes in (y, √x] to iterate, but primal lookups are fast.
Net: 1T 0.168s → 0.120s, 10T 0.781s → 0.560s.

### Current State
V4 LMO: 1T 0.120s, 10T 0.560s — beats V3 (0.168s / 1.190s) by 1.4× / 2.1×.
Main bottleneck: S2 (78% at 10T). Next opportunities: parallelism, larger segments.
### Parallel P2 (WIN)
P2 computes pi(x/p) for independent primes p in (y, sqrt(x)].
Switched from sequential loop to par_iter().map().sum() via rayon.
Result: 1T 0.120s -> 0.098s, 10T 0.560s -> 0.480s (~18% faster).
P2 was 28% of time; parallelizing across 24 threads nearly eliminates it.

### Tracked Total in BitSieve (FAILED)
Added a running 	otal field to BitSieve, decrementing in cross_off().
The branch to check if bit was set causes pipeline stalls.
Branchless AND NOT is faster. Result: 1T 0.120s -> 0.140s (17% SLOWER). Reverted.

### Segment Size Cap at 64K (NO EFFECT)
Capped segment_size at 65536. No measurable difference within noise. Reverted.

### Incremental Count (count_delta) — BIG WIN
Key insight: in special leaves loops, m decreases -> xpm increases (hard leaves),
q decreases -> xpq increases (easy leaves). Positions are monotonically ascending.
Instead of scanning from position 0 for each count(), track running_count and add
only the delta between consecutive positions via count_delta(prev_pos, pos).

Bug found: word >> (b0+1) when b0=63 causes shift-by-64, which wraps to shift-by-0
in Rust release mode. Fixed with explicit mask check.

Result: 1T 0.098s -> 0.091s, 10T 0.480s -> 0.396s (18% faster at 10T).
Savings scale with x because more leaves = more amortized delta scans.

### Current State
V4 LMO: 1T 0.091s, 10T 0.396s — beats V3 (0.168s / 1.190s) by 1.8x / 3.0x.
Main bottleneck: S2 (still dominant). Next: parallel S2, wheel-30, precompute m indices.

### Pre-sieve Template (MASSIVE WIN)
Replaced 6 individual cross-off loops for primes 2,3,5,7,11,13 with a precomputed
30030-bit template (period = LCM(2..13)). Template stored doubled (60060 bits = 7.5KB)
for wrapping safety. Applied via word-aligned AND + POPCNT total adjustment.
21K scalar operations replaced by ~256 word-level AND ops per segment.
Result: 1T 0.078s -> 0.042s, 10T 0.340s -> 0.175s (1.9x speedup).

First version had a wrapping bug at the period boundary — single-period storage
with manual bit splicing produced small errors. Fixed by storing 2 full periods.

### Alpha Re-tune to 2.0 (WIN)
With pre-sieve template, the balance shifted. Pre-sieve was 50% of S2 time;
eliminating it makes smaller y (less P2) more attractive.
Alpha 2.0: 1T 0.039s, 10T 0.162s. (Was 2.5 before template)

### Current State
V4 LMO: 1T 0.040s, 10T 0.163s — beats V3 (0.168s / 1.190s) by 4.2x / 7.3x.
Main bottleneck: S2 cross-off loop for primes p_7..p_max_b. Next: parallel S2.

### Concurrent S2+P2 (WIN)
Used std::thread::scope to run P2 in background while S2 runs on main thread.
P2 was 20% of total time; now completely overlapped with S2.
Result: 1T 0.040s -> 0.033s, 10T 0.163s -> 0.137s (18% faster).

### Batched cross_off_step (FAILED)
Tried building per-word masks for primes < 64 to batch crossings within each u64.
10% slower due to mask building overhead and extra branches.
Most primes p >= 64 have only 1 crossing per word anyway. Reverted.

### Alpha Re-tune to 2.0 (WIN)
With pre-sieve template, alpha=2.0 is optimal again (was 2.5).
Pre-sieve cost was what made higher alpha attractive; now eliminated.

### Current State
V4 LMO: 1T 0.031s, 10T 0.134s - beats V3 (0.168s / 1.190s) by 5.4x / 8.9x.
S2 is now 100% of bottleneck (P2 is free via concurrency).

### Combined init_sieve (WIN - marginal)
Merged reset() + template.apply() + cross_off(0) into single init_sieve() pass.
Eliminates one full pass over sieve words. Removed dead reset() method.
Marginal ~2% improvement. 1T: 0.033s -> 0.034s (noise), 10T: 0.137s -> 0.135s.

### Larger Segment Size 128K bits (WIN - 5%)
Default segment size sqrt(z) was only 16K at 10T (2KB as bytes). Very small!
Increasing minimum to 128K bits (16KB) reduces per-segment overhead:
- 10T: 14170 segments -> 1780 segments (8x fewer!)
- Each segment: init_sieve, prime start calculations, phi bookkeeping
- 256K (32KB) was worse due to L1 pressure (48KB cache)
Result: 1T 0.034s -> 0.031s, 10T 0.142s -> 0.134s.

### Analysis: What's left?
S2 cross-off loop is the only bottleneck now. At 10T with alpha=2.0:
- z = 232M, segment_size = 128K, num_segments ≈ 1780
- Each segment processes primes from p_7=17 up to max_b
- Cross-off for small primes (17-61): many crossings per segment, good throughput
- Cross-off for large primes: 0-1 crossings, minimal cost

Options considered:
1. Extend template to prime 17 (c=7): PhiTiny cache would be 4MB (too large for L2)
   Would need to decouple template c from PhiTiny c. Complex.
2. Parallel S2: Would need phi precomputation per thread. Major refactor.
3. Reduce divisions: x/(prime*m) costs ~25 cycles. Could precompute x/prime once per b.
4. Fenwick tree: O(log n) vs O(n) for count, but cross_off overhead increases too.

### Precompute x/prime (WIN - 6%)
Hoisted x/prime out of inner loops. floor(x/(a*b)) = floor(floor(x/a)/b) exactly.
Eliminates one redundant division per leaf. 10T: 0.134s -> 0.126s.

### S2 Profiling Results (10T, α=2.0)
Detailed timing breakdown:
- init_sieve: 2ms (1.5%)
- hard_leaf: 10ms (8%)
- easy_leaf: 57ms (44%) ← SURPRISE! Not cross_off!
- cross_off: 58ms (46%)
Total S2: ~127ms
Key insight: easy leaves dominate equally with cross_off. Need to optimize both.

### 4x Unrolled Cross-off (WIN - 7%)
Unrolling the `while k < seg_len { cross_off(k); k += p; }` loop by 4 reduces
loop control overhead (branch, increment, compare) by 75%.
8x unroll was SLOWER (code bloat / register pressure / I-cache misses).
10T: 0.126s -> 0.117s.

### Failed Attempts
- Barrett reciprocal table for ALL m values (344KB): 4% SLOWER due to L2 cache pressure.
  The table displaced mu/lpf/primes from cache.
- Wheel mod 30 for hard leaf m iteration: overhead ≈ savings (wash).
  The mu[m] check is so fast (1 byte compare) that skipping it isn't worth wheel overhead.
- Odds-only m iteration: marginal at best, noise at 10T.
- Alpha retuning (1.5-3.0): 2.0 still optimal.
- Unsafe indexing in leaf loops: compiler already elides bounds checks in release mode.

### Barrett Fast Division for Primes Only (WIN - 8%)
KEY INSIGHT: Full Barrett table (y+1 entries = 344KB) causes L2 cache pressure.
Primes-only table (π(y) entries ≈ 4600 = 37KB) fits in L1!
Branchless correction: q = mulhi(n, recip) + (n - q*d >= d).
Replaces 25-cycle hardware DIV with ~12-cycle multiply-high in easy leaf inner loop.
10T: 0.117s -> 0.110s.

### Position Reuse for Easy Leaves (WIN - marginal)
When consecutive primes give the same floor(x/q) position, skip count_delta query
and reuse previous count. Saves sieve access for duplicate positions.

### Current State
V4 LMO: 1T 0.027s, 10T 0.110s - beats V3 (0.168s / 1.190s) by 6.2x / 10.8x.
S2 breakdown: easy_leaf ≈ 44%, cross_off ≈ 46%, hard_leaf ≈ 8%, init ≈ 2%.
Total speedup from initial V4 sieve baseline: 0.366s -> 0.027s = 13.6x at 1T.

### Inline Cross-off with Deferred Total (WIN - marginal)
Manually inlined 4× unrolled cross-off using raw pointers. Delta accumulated in local
variable, applied once per prime. Breaks serial dependency on sieve.total.
Word-level batching (mask per word for p<64) was 10% SLOWER — inner loop overhead and
XOR+POPCNT per word exceeded benefits.
Result: 10T 0.110s -> 0.109s (marginal, within noise).

### Summary of Session Wins
From committed baseline (0.033s / 0.137s at 1T / 10T):
1. Combined init_sieve: 0.034s / 0.135s (marginal)
2. Larger segments (128K): 0.031s / 0.134s (-5%)
3. Precompute x/prime: 0.029s / 0.126s (-6%)
4. 4× unrolled cross-off: 0.027s / 0.117s (-7%)
5. Easy leaf pos reuse: 0.027s / 0.116s (marginal)
6. Barrett for primes (37KB): 0.027s / 0.110s (-5%)
7. Deferred total cross-off: 0.026s / 0.109s (marginal)
Total session improvement: 0.033s → 0.026s (1.27×) / 0.137s → 0.109s (1.26×)

### Failed Attempts This Session
- Barrett for ALL m values (344KB table): 4% SLOWER (cache pressure)
- Wheel mod 30 for hard leaf m: wash (overhead ≈ savings)
- Odds-only m iteration: wash
- Alpha retuning: 2.0 still optimal
- Unsafe indexing: compiler already elides checks
- 8× cross-off unroll: SLOWER (code bloat)
- Word-level cross-off batching: 10% SLOWER

### Parallel S2 via Delta-Phi Correction (MASSIVE WIN - 1.8×)
Key insight: phi[b] is the only cross-segment dependency in S2. Within each segment,
sieve state is deterministic (template + same cross-off order). The leaf formulas
s2 -= mu[m] * (phi[b] + count) can be decomposed: count is segment-local and correct,
phi[b] is the only error. Each thread tracks correction coefficients:
- Hard: coeff[b] -= mu[m] per leaf
- Easy: coeff[b] += 1 per leaf
After join, prefix-sum of phi totals gives true phi at each thread boundary.
Correction = Σ_b prefix_phi[b] × coeff[b]. Exact, O(primes × threads).

Cross-off starting positions computed from scratch: first_mul = ceil(max(low,p)/p)*p.
No next[b] state needed across threads. Serial fallback for ≤2 segments.

Result: 10T 0.110s → 0.062s (1.8× speedup!). All tests pass.
S2 was 75ms serial, now ~40ms parallel with 24 threads.
1T: 0.026s → 0.015s. 100B: 0.007s → 0.004s.

Failed before this: cyclic AND masks for small primes (p<64) — mask precomputation
per segment costs as much as original cross-off. 2× SLOWER. Reverted.

### Chunk Count Tuning + Truncated Correction (WIN - 10%)
Swept chunk multipliers: 4×=0.055s, 6×=0.053s, 8×=0.051s, 10×=0.050s, 16×=0.053s,
per-segment=0.080s. Per-segment causes 1771 chunks × 4600 primes correction = too much.
Truncated correction (only iterate to max_b_seen per chunk) helps but not enough.
8× chosen as optimal. 10T: 0.062s → 0.050s.

### Alpha Re-tune to 1.6 (WIN - 16%)
With parallel S2, lower alpha = more segments = better work-stealing parallelism.
Sweep: α=1.0 blocked, 1.2=0.052s, 1.4=0.044s, 1.5=0.042s, **1.6=0.041s**, 1.8=0.044s, 2.0=0.050s.
α=1.6 is optimal for parallel S2. Previous α=2.0 was optimal for serial S2.
10T: 0.050s → 0.042s. 1T: 0.013s → 0.010s.

### Current standings
V4 LMO: 1T 0.010s, 10T 0.042s - beats V3 by 16.8x / 28.3x.
Total speedup from initial V4 baseline: 0.366s → 0.010s = 36.6x at 1T.

## Session: Continued V4 Optimization (Allocation + Contention Analysis)

### Attempt 19: mimalloc Global Allocator (WIN)
Profiling revealed shocking allocation overhead: 129ms alloc vs 147ms work across 768 chunks.
Each chunk allocates BitSieve (16KB) + phi (29KB) + coeff (29KB) = 74KB. Total 57MB.
Windows default heap is slow under multi-threaded contention.

Added mimalloc as global allocator. Alloc overhead dropped from 129ms to ~15ms (8.6x faster).
10T: 0.042s → 0.040s. Modest but real improvement.

### Contention Analysis
Ran S2 and P2 both separately and concurrently:
- S2 alone (all rayon threads): 42ms → 35ms with mimalloc
- P2 alone: 37ms
- Concurrent S2+P2: 47ms → 40ms with mimalloc
- Contention overhead: only 4.8ms — P2 sieve build (25ms) fully overlaps with S2

### Parallel Efficiency Deep Dive
S2 serial: 105ms. S2 parallel (24 threads): 35ms. Speedup: 3.0x.
Only 12.5% parallel efficiency! Root causes identified:
1. **Max chunk bottleneck**: Heaviest chunk (segments 0-2) takes 23ms alone
   - This chunk processes all 3588 primes with max_b at maximum
   - Even with infinite threads, S2 can't be faster than 23ms
2. **Work distribution extremely skewed**: First 10% of chunks do ~80% of work
   - Cross-off per segment 0: ~173K operations (3588 primes)
   - Cross-off per last segment: ~500 operations (few primes)

### Failed attempt: Flat pre-allocated arrays
Tried replacing per-chunk Vec allocations with flat pre-allocated arrays (1 alloc instead of 768).
Result: no measurable difference. The zeroing of 44MB flat array takes as much time as
many small allocations with mimalloc.

### Key insight for further optimization
The 23ms max chunk is the fundamental bottleneck. To break through, need either:
1. Split the heavy segments into sub-problems (hard - leaves depend on sieve state)
2. Parallelize within a single segment (cross-off different primes in parallel)
3. Reduce algorithmic work in heavy segments (different formula variant)
4. Or find a completely different parallelization strategy

### Current standings
V4 LMO: 1T 0.010s, 10T 0.040s - beats V3 by 16.8x / 29.8x.

## V4 Optimization 20: Pi-Formula for Segment 0 Easy Leaves (WIN)

### Analysis
Profiling revealed segment 0 has 4.7M easy leaf iterations taking 19.5ms.
For these leaves, primes[b-1]^2 >= high (segment upper bound), so the identity
phi(n, b-1) = 1 + max(pi(n) - (b-1), 0) applies.

Key insight: most easy leaves at 10T have b >> pi(xpq), so phi = 1. These can be
batch-counted in O(1) per prime without iterating individual leaves.

### Bug Fix
Initial implementation used pi(n) - b + 2 without the max(., 0) guard.
When b-1 > pi(n), phi should be 1 (not negative). Fixed to: 1 + max(pi(n) - (b-1), 0).

### Implementation
1. Build small pi table for [0, segment_size] at start of compute_s2 (~50us)
2. For segment 0 easy leaves where primes[b-1]^2 >= high:
   a. Compute l_phi1_boundary = pi[x/(p*primes[b-1])]
   b. Batch-count leaves with l > l_phi1_boundary (all phi=1, O(1) per prime)
   c. Iterate remaining leaves with pi table lookup (O(1) per leaf vs O(pos/64) for sieve)
3. Non-qualifying segments use original sieve-based counting

### Result
10T: 0.040s -> 0.036s (~10%). 1T: 0.010s -> 0.008s (~20%).
Committed as 86c44da, pushed to GitHub.

### Why It Works
- Eliminates 572-word sieve.count() scan per prime (1ms saved)
- Eliminates count_delta for 4.7M leaves (4.3ms saved with pi lookup vs ~0)
- Batch counting skips Barrett division for ~30% of leaves (~1ms saved)
- Total savings: ~6ms from 19.5ms easy leaf cost


## V4 Optimization 21: Alpha Re-tune to 2.2 (WIN)

With pi-formula reducing per-leaf cost in segment 0, higher alpha became optimal.
At alpha=2.2: y=47,397, z=211M. More primes but pi-formula handles them.
10T: 0.036s -> 0.031s.
Committed as 2bdf559, pushed.

## Next Investigation: Segment Weight Distribution

Need to understand where time is spent now. Is segment 0 still the bottleneck?
With alpha=2.2: y is larger, z is smaller. Fewer segments (1614 vs 2213).
Segment 0 has even more easy leaves (more primes from larger y).
But pi-formula + batch counting handles them efficiently.

## V4 Optimization 22-24: Parameter tuning

Adaptive segment size (2^17 for ≤1T, 2^19 for 10T), chunk count 6× threads,
and p³ full batch. Incremental wins: 10T 0.031s -> 0.028s.

## V4 Optimization 25: Parallel P2 Sieve (MAJOR WIN)

### Profiling Discovery
Added detailed timing instrumentation to count_primes and compute_s2.
Results at 10T:
- S2 setup: 0.14ms, pi_table: 1.96ms, par_iter: 16.58ms, correction: 0.05ms
- P2: **26.96ms** — THIS IS THE BOTTLENECK!
- Chunk 0 (segment 0): 16.49ms (2 segments)

Key insight: P2 was taking almost as long as total time because
primal::Sieve::new(211M) is completely single-threaded. P2 and S2 both
use rayon's thread pool via thread::scope, causing contention.

### Failed Approaches
1. **Precompute seg0 easy leaves in parallel**: Separate par_iter before main par_iter.
   Both are sequential. Added rayon dispatch overhead without reducing total time.
   Reverted.

2. **Word-batched cross-off (p < 64)**: Accumulate bit masks for same u64 word.
   Break-even: inner loop overhead matches load/store savings. Reverted.

3. **Reduce segment_size to L1**: Capped at 2^18 (sieve = 32KB fits L1d).
   WORSE: more segments means more times processing 4903 primes. Reverted.

4. **One chunk per segment**: Good for load balancing but 403 chunks has
   higher overhead. Similar to 6× chunks. Reverted.

5. **Sequential P2 then S2**: P2 with full rayon: 13ms. S2 with full rayon: 19ms.
   Total: 32ms > concurrent 28ms. Worse because P2+S2 > max(P2,S2).

### Solution: Custom ParallelPiSieve
Replaced primal::Sieve with custom ParallelPiSieve:
- Odd-number bitmap: bit i represents number 2i+1
- Built in parallel via rayon::par_chunks_mut (each chunk independently sieved)
- Prefix-sum array of u32 popcount values for O(1) π(n) queries
- Cross-off primes only up to √limit (~14.5K at 10T)

P2 standalone: 27ms -> 13ms (2× faster with full rayon access)
P2 concurrent with S2: ~20ms (sharing rayon threads via work-stealing)

Overall: 10T 0.028s -> 0.022s best (21% improvement)
         1T 0.007s -> 0.006s best (14% improvement)

Committed as 0df91c6, pushed.

### Why It Works
primal::Sieve::new() builds the entire sieve on a single thread, taking ~25ms
for 211M numbers. Our ParallelPiSieve distributes the cross-off work across
rayon's thread pool. Even when sharing threads with S2 (via work-stealing),
rayon naturally balances: S2 tasks that finish early pick up P2 chunks.

### Current Standings
V4 LMO best-of-10:
| Range       | Time    |
|-------------|---------|
| 1 Billion   | 0.0009s |
| 10 Billion  | 0.002s  |
| 100 Billion | 0.003s  |
| 1 Trillion  | 0.006s  |
| 10 Trillion | 0.022s  |

Total speedup from initial V4 baseline: 0.366s → 0.006s = **61×** at 1T.


## Session: V4 Optimization Sprint — PGO and Adaptive Alpha

### PGO (Profile-Guided Optimization) — FAILED

Tried full PGO pipeline: instrumented build → profile collection → profile-use rebuild.
- First attempt lost target-cpu=native due to RUSTFLAGS env var overriding .cargo/config.toml
- Second attempt with both flags: still no improvement (within noise at all scales)
- Root cause: LTO=fat + codegen-units=1 already captures most PGO benefits. Hot loops are manually unrolled with minimal branching — nothing for PGO to improve.

### Adaptive Alpha — MASSIVE WIN (Optimization 27)

Key discovery: optimal alpha varies dramatically with input size.

Alpha sweep results:
| Alpha | 10T best | 100T best | 1Q best  |
|-------|----------|-----------|----------|
| 2.2   | 0.022s   | 0.103s    | 2.21s    |
| 2.4   | 0.026s   | 0.099s    | ~1.95s   |
| 3.0   | 0.027s   | 0.110s    | 1.58s    |
| 3.5   | 0.028s   | 0.122s    | 1.25s    |
| 4.0   | 0.031s   | 0.128s    | 1.06s    |
| 5.0   | 0.037s   | 0.133s    | 0.83s    |
| 6.0   | 0.042s   | 0.170s    | 0.79s    |
| 8.0   | ~0.05s   | 0.191s    | 0.94s    |

Why it works: For larger x, S2 segments have longer inner loops, so per-prime overhead
(starting position calc, phi tracking) is better amortized. Higher alpha → larger y, fewer
segments (z=x/y smaller), less total S2 work. Even at α=6 for 1Q, there are still 3200+
segments — plenty for parallelism.

Formula chosen: alpha = f(log10(x))
- x ≤ 10^13: α = 2.2
- 10^13 < x ≤ 10^14: linear 2.2→2.4
- x > 10^14: α = 2.4 + 3.6*(log10(x)-14) → 6.0 at 1Q

Results: 100T 0.103→0.099s (4%), 1Q 2.21→0.793s (64%!). No regression on smaller ranges.

---

## V4 Session: 10Q/100Q Benchmarks and Alpha Retuning

### Segment Size Cap Attempt (FAILED)
- Tried capping segment_size at 1M (2^20) to keep sieve in L2 cache
- At 1Q: S2 went from 815ms to 890ms (9% WORSE)  
- More segments = more template init overhead and correction pass work
- At 100T: no change (already using 2M segments)
- **REVERTED**

### Frozen Sieve Cross-off (Opt 28) — Marginal Win
- When prime p > sqrt(high), all composite multiples already cleared by smaller primes
- Only p itself needs clearing (O(1) vs O(segment_size/p))
- Mathematical proof: for c = k*p in [low, high), smallest factor ≤ sqrt(c) < p
- Result: ~3-7% improvement at 100T, within noise at other scales
- Kept for architectural correctness

### 10Q/100Q Benchmarks Added
- 10Q (10^16): π(10^16) = 279,238,341,033,925 — 5.68s
- 100Q (10^17): π(10^17) = 2,623,557,157,654,233 — 34.33s
- Both verified correct

### Alpha Retuning (Opt 29) — 9% Win at 10Q
- Swept alpha 6-20 for 10Q. Previous formula gave α=9.6, optimal is α=13
- Swept alpha 10-30 for 100Q. Optimal α≈16, close to previous formula α=13.2
- Extended formula with new breakpoints:
  - 10^15→10^16: slope 7.0 (was 3.6)
  - >10^16: slope 3.0
- 10Q: 6.25s → 5.68s (9% faster)
- 100Q: ~same (34.65s → 34.33s, within noise)

### S2 Profiling Insights at 1Q
- S2 = 815ms, P2 = 238ms (S2 is 3.4× bottleneck)
- segment_size = 4M (512KB sieve, exceeds L1 48KB but fits L2 2MB)
- y = 600K, z = 1.67B, ~400 segments across 144 chunks
- Segment 0 processes 49K primes; segment 1 drops to ~1832 (massive imbalance)

### Key Insight: Odd-only Sieve Potential
- Current S2 sieve covers ALL integers, but even positions are ALWAYS zero after template
- Switching to odd-only sieve would: halve memory (256KB, closer to L1), halve cross-off iterations, halve template init
- Expected benefit: ~30-40% reduction in S2 time
- This is the next major optimization to implement
---

### Odd-only S2 Sieve (Opt 30) — 5-14% Win
- Switched sieve from all-integer to odd-only representation
- Half the memory, half the cross-off iterations, half the template
- Template period: 15015 (lcm of 3,5,7,11,13) vs 30030 (lcm of 2,3,5,7,11,13)
- Key helper: int_to_odd_bp(n, low) = (n - low - 1) / 2
- Edge case: xpq == low (even) → count = 0, contribution = phi[b] only
- Results: 10T 14%, 100T 5%, 1Q 5%, 10Q 4%, 100Q 2%
- Committed: 2005152

---

### P2 Prefix u32→u64 Fix + Max i64 Benchmark (Opt 31)
- ParallelPiSieve stored prefix counts as Vec<u32>, overflows at π(z) > 4.3B
- At x = 2^63-1: z ≈ 200B, π(z) ≈ 7.7B → u32 overflow
- Fix: Vec<u32> → Vec<u64> in struct, construction, and accumulation
- Memory doubles for prefix (12.5GB at 2^63-1 scale), but 96GB RAM sufficient
- Total P2 memory at 2^63-1: bitmap 12.5GB + prefix 12.5GB ≈ 25GB
- **π(2^63-1) = 216,289,611,853,439,384** computed in **939.2 seconds** (15.65 min)
- Verification: Li(2^63-1) ≈ 2.17 × 10^17, ratio vs π(10^18) = 8.74 (expected ~8.75)
- All existing benchmarks verified correct, no regression
- Committed: 8fb32fe

#### Next optimization targets:
- S2 is 3-4× bottleneck at all scales ≥ 1Q
- Wheel-30 S2 sieve: 8/30 vs 1/2 → ~47% less cross-off and memory
- Gourdon's AC algorithm: fundamentally better O(x^{2/3}/log²x)
- Bucket sieve: pre-sort primes by next contributing segment

---

### P2 u64 Fix and π(2^63-1) Computation (Opt 31)
- Fixed ParallelPiSieve prefix table: Vec<u32> → Vec<u64>
- π(2^63-1) = 216,289,611,853,439,384 computed in 939.2 seconds (15.65 min)
- Committed: 8fb32fe

---

### Failed: Wheel-30 S2 Sieve
- Full implementation: wheel-30 constants, compute_wheel_steps, first_wheel_multiple,
  int_to_wheel_bp, PreSieveTemplate for primes 7/11/13
- All correctness tests pass
- 3-21% SLOWER across all scales
- Root cause: variable stride (k += steps[ci]) defeats hardware prefetcher
  Odd-only (k += p constant stride) allows perfect prefetch prediction
- Division by 30 for position mapping is ~5× more expensive than shift-by-1
- Loss of 4× unrolling effectiveness with non-uniform steps
- Committed failure doc: 07d3733

### Failed: Batch Counting Extension
- Extended pi-formula batch (p³ ≥ x) from segment 0 to all segments
- 2-79% SLOWER: condition check overhead (u128 multiply per prime per segment)
  with zero benefit since p³ ≥ x rarely triggers at scales < 10Q

### Failed: Segment Size Cap
- Tried 2^22 (sieve=256KB) and 2^24 (sieve=1MB) caps
- More segments = more template init + phi tracking + correction pass overhead
- Only 100Q improved with 2^22 cap (+5%); everything else worse

### Failed: Chunk Count Tuning
- Tried nchunks = threads × 4 and × 16 (original: × 6)
- × 16: correction pass overhead dominates (9-89% worse)
- × 4: insufficient work-stealing (9-13% worse)
- × 6 is the sweet spot

### Conclusion
The V4 LMO implementation is now highly optimized at the micro-level. Remaining
improvements require algorithmic changes:
1. Gourdon's AC algorithm (O(x^{2/3}/log²x) — fundamentally better)
2. Different S2 decomposition (avoid segmented sieve entirely)
3. Hardware-level: AVX-512 when Arrow Lake successor supports it

---

## Session: V1/V2/V3 Large-Scale Correctness Fixes + Benchmark Extension

### Barrett Reciprocal Overflow Discovery (Critical Bug in All Three Versions)

Running V2/V3 at 100T+ revealed two precision bugs in the reciprocal division:

1. **small[] update overflow** (40-bit reciprocal in u64):
   - `recip_p = ceil(2^40/p)`, used as `(j * recip_p) >> 40`
   - At 100T: v=10^7, j*recip_p can reach 10^19 (borderline u64 max 1.84×10^19)
   - At 1Q: v=3.16×10^7, product up to 3.16×10^19 — overflow!
   - At 10Q: v=10^8, product up to 10^20 — massive overflow
   - Fix: upgrade to 48-bit reciprocal with u128 multiply

2. **large[] update Barrett overestimate** (64-bit reciprocal, V2/V3):
   - `recip[j] = ceil(2^64/j)`, used as `(n_div_p * recip[j]) >> 64`

---

## Session: V6 Enhanced DR Implementation + Optimization

### V6 Opt 0 — Segmented Pi Table (Baseline)

Implemented V6 with Gourdon-inspired segmented π-table processing for S2_easy.
Key innovation: instead of requiring full π table in L3 cache, divide into L2-sized segments.
Removed y=9M cap from V5 — V6 handles any y size efficiently.

- Architecture: dispatch between direct (y≤9M, pi fits L3) and segmented (y>9M)
- Segmented: parallel over segments via rayon, each segment 512K entries = 2MB
- Results: 1 Quintillion 113.4s (V5: 172.4s = 1.52× faster), Max i64 547.3s (V4: 939.2s)

### V6 Opt 1 — Alpha Tuning + Segment Size

Two complementary tuning changes yielding 27% combined speedup at 1 Quintillion.

**Alpha tuning**: Swept alpha 12-50 at 1 Quintillion. Optimal alpha=23 (was 19).
Higher alpha → larger y → smaller z → fewer S2_hard sieve segments.
With segmented S2_easy, larger y costs almost nothing in S2_easy.
Updated curve: log_x 17-18 ramps 16→23, log_x>18: 23+5*(log_x-18).

**Segment size optimization**: Swept 32K to 2M entries at 1 Quintillion.
Results: 128K entries (512KB) optimal — S2_easy 50.8s vs 98.1s at 512K.
At 128K, S2_easy finishes early (50s), freeing rayon threads for S2_hard.
S2_hard drops from 98s to 81s thanks to getting more threads sooner.
Total: 82.6s (was 99.1s with 512K, was 113.4s at Opt 0).

**Alpha+segment interaction**: Re-tested alpha 19-28 with 128K segments.
Alpha 21-28 all show S2_hard=81s bottleneck. Alpha=21 marginally best (81.8s).
Alpha=23 gives 82.1s — within noise. Kept alpha=23 for balance at Max i64.

Combined results:
- 1 Quintillion: 113.4s → 82.6s (27% faster)
- Max i64: 547.3s → 502.4s (8% faster)
- No regressions at smaller scales

**New bottleneck**: S2_hard at ~81s. S2_easy now finishes in 50s and
threads migrate to S2_hard, but it still takes 81s total.

### S2_hard Optimization Attempts (All Unsuccessful)

**S2_hard segment size cap (L2-fitting)**: Tried capping segment_size at 2^24
(16M numbers = 1MB sieve, fits L2). Result: WORSE (88.7s vs 82.5s).
Larger segments amortize per-segment overhead better, and sieve access is
sequential enough for L3 to work well even at 4.2MB.

**S2_hard nchunks tuning**: Swept multiplier 3×-24× (72-576 chunks).
Results: 6×-12× all within 1% (81.8-82.4s). Current 6× is in sweet spot.
Very few chunks (3×) suffer load imbalance; very many (24×) add overhead.

**S2_hard target_segs tuning**: Swept 8-128× (controls segment_size).
TSEGS=64 (32MB segments, 2MB sieve) gave 82.7s, same as default 32× (85.8s).
Larger segments (256MB) much worse (105s) due to first-segment load imbalance.

**BitSieve hierarchical counters**: Attempted adding block_sums (popcount per
64 u64 words) to make count() O(nblocks) instead of O(nwords). The approach
works but: (a) maintaining block_sums during sieve crossing adds 57% overhead
to the hot 4x-unrolled loop, (b) lazy recomputation adds complexity, and (c)
the saving from count() is marginal because count_delta (the common case) is
already fast for nearby positions. Reverted.

**Root cause analysis**: S2_hard is dominated by Type 1 leaves (b ≤ π(√y) ≈ 648).
For each (b, segment) pair, the inner loop iterates over all m ∈ [min_m, max_m]
checking mu[m]≠0 and lpf[m]>p. First segment has max_m=23M, most m values are
skipped (mu=0 or lpf too small). This is fundamental to the algorithm.

**Conclusion**: S2_hard at ~81s is near-optimal for the DR algorithm on this
hardware. Further gains require algorithmic changes (Gourdon decomposition).
   - Barrett guarantee: gives floor(n/d) or floor(n/d)+1
   - Overestimates when n*(d-1) >= 2^64
   - At 10T: errors happen but are rare enough to cancel
   - At 100T+: errors accumulate (2 at 100T, 17 at 1Q, 30 at 10Q)
   - Fix: `div_recip()` helper — compute via reciprocal, then verify q*d <= n

3. **V1 fast_div overflow** (same Barrett bug):
   - `fast_div(seg_start + p - 1, recip)` overestimates k_min for primes > 184K at 100T
   - Causes 99,919 composites to be missed (overcounting primes)
   - Fix: add correction `q - (q*p > n) as u64`

**Key insight**: Barrett overestimate always gives q+1, never q-1. The correction
check `q*d > n` is a single u64 multiply + compare, with near-zero overhead since
the overestimate is rare and the branch predicts well.

### Performance Impact of Barrett Correction
- V2 at 10T: 1.23s → 1.39s (+13% overhead from correction) — acceptable
- V3 at 10T: 1.19s → 1.33s (+12% overhead) — acceptable
- V1 at 10T: 127.1s → 132.4s (+4% overhead) — negligible
- Tried u128 vs u64 for correction check: u128 was ~20% slower due to register spills
  u64 check is safe because q*d ≈ n < 2^64 (no overflow possible in our range)

### New Benchmarks Achieved
| Version | 100T | 1Q | 10Q |
|---------|------|----|----|
| V1 | 2389.2s ✓ | — (impractical) | — |
| V2 | 8.07s ✓ | 42.51s ✓ | — (too slow) |
| V3 | 7.83s ✓ | 40.57s ✓ | 208.33s ✓ |
| V4 | 0.091s | 0.755s | 5.43s |

All V4 reference values verified correct against V2/V3 independent implementations.

### Commits
- 312ac49: V2/V3 Barrett fix + extend benchmarks to 10Q
- (u64 optimization commit): Optimize correction from u128 to u64
- 1a2718d: V1 Barrett fix + 100T benchmark (2389s)
- 1505871: Clean timing update for V2/V3

---

## Session: Planning V5+ Algorithm Implementations

### Algorithm Landscape Analysis

Researched prime counting algorithms beyond LMO. Key findings:

| Algorithm | Complexity | Practical Status |
|-----------|-----------|-----------------|
| V1 Segmented Sieve | O(N) | ✅ Done |
| V2 Lucy_Hedgehog | O(N^{3/4}/ln N) | ✅ Done |
| V3 Meissel-Lehmer | O(N^{2/3}) | ✅ Done |
| V4 LMO | O(N^{2/3}/ln N) | ✅ Done |
| **V5 Deleglise-Rivat** | O(N^{2/3}/ln²N) | **Next target** |
| V6 Gourdon | O(N^{2/3}/ln²N) better constants | After V5 |
| V7 Analytic (Lagarias-Odlyzko) | O(N^{1/2+ε}) | Extreme difficulty |

### Deleglise-Rivat Key Differences from LMO

The DR algorithm uses the same base formula as LMO:
  π(x) = S1 + S2 + π(y) - 1 - P2

But splits S2 into three parts:
- **S2_trivial**: Primes b where x/(p_b · p_{b+1}) < y — contribution is a simple
  closed-form sum. Essentially free to compute.
- **S2_easy**: Two-prime products where both primes allow direct π-table lookup.
  No sieve needed! Can be computed from precomputed π tables. Highly parallelizable.
- **S2_hard**: The remaining special leaves that DO require sieve computation.
  Same approach as LMO's S2 but covers a much smaller range.

The magic: by increasing α (the tuning parameter), more work shifts from S2_hard
(expensive sieve) to S2_easy (cheap table lookups). This is why DR is faster —
it avoids sieving for the majority of special leaves.

Kim Walisch's primecount (C++) is the reference implementation. Uses OpenMP
parallelization. Default algorithm for all large computations.

### Implementation Plan for V5
1. Start from V4's proven infrastructure (BitSieve, PreSieve, ParallelPiSieve, etc.)
2. Implement S2_trivial (simple sum)
3. Implement S2_easy (π-table lookups, parallelizable)
4. Implement S2_hard (reduced version of V4's S2 sieve)
5. Get correctness first, then optimize
6. Key optimization: tune α to minimize S2_hard work
## V5 Deleglise-Rivat Implementation

### Algorithm Design (from primecount study)

Formula: pi(x) = S1 + S2_easy + S2_hard + pi(y) - 1 - P2

Key innovation over V4 (LMO): for special leaves where x/(p_b*p_l) <= y and p_b > sqrt(y),
phi(n, b-1) = pi(n) - b + 2. This bypasses the segmented sieve for easy leaves.

### Bugs Found & Fixed

1. Type 1 loop filtered by xpm > y - ALL type 1 leaves need sieve for b <= pi(sqrt(y))
2. S2_trivial/S2_easy overlap - merged S2_trivial into S2_easy
3. Easy/hard boundary gap (xpq = y) - changed to xpq >= y for hard side
4. Trivial easy leaves (phi=1) not counted separately

### Baseline Performance (V4 alpha, unoptimized)
- V5 is 1.6-4.2x slower than V4 across all ranges
- S2_hard is single-threaded (main bottleneck)
- All test cases correct through 1 Quintillion

### V5 Opt 1: Parallel S2_hard (2.2x speedup)
- Delta-phi correction technique from V4
- Serial fallback for small inputs
- Large-prime and sqrt_high optimizations for cross-off

### V5 Opt 2: Concurrent S2_easy + Alpha Retuning (34% at 10Q)
- S2_easy runs on separate thread, hidden behind S2_hard
- Alpha steeper ramp at 100T: 2.2->3.0 (was 2.2->2.4)
- V5 now BEATS V4 at 1Q+ scales:
  - 1Q: 0.636s (V4: 0.755s) = 1.19x faster
  - 10Q: 3.74s (V4: 5.43s) = 1.45x faster
  - 100Q: 23.88s (V4: 33.63s) = 1.41x faster

### V5 Opt 3: Pi-formula fast path for segment 0 (32% at 100T)
- Precompute global pi(n) table for segment 0, use phi(n,b-1) = 1+max(pi(n)-(b-1),0)
- Avoids sieve counting for type 2 leaves where primes[b-1]^2 >= high
- Batch trivial phi=1 leaves when p^3 >= x (O(leaves) without pi-lookup)
- Capped at 16M to avoid multi-GB allocation at large scales (segment_size grows with x)
- Bug found during development: 4GB global_pi allocation at 1 Quintillion caused massive slowdown
- Results: 100T 0.135s->0.092s (32%), 1Q 0.636s->0.539s (15%), 10Q 3.74s->3.55s (5%)
- V5 now beats V4 at ALL scales 100T+:
  - 100T: 0.092s (V4: 0.112s) = 1.22x faster
  - 1Q: 0.539s (V4: 0.770s) = 1.43x faster
  - 10Q: 3.55s (V4: 5.64s) = 1.59x faster
  - 100Q: 24.58s (V4: 35.80s) = 1.46x faster
  - 1 Quintillion: 206.4s (V4: ~192s) — slightly slower, needs investigation

### V5 Opt 4: Y-cap for L3 cache efficiency (16% at 1 Quintillion)
- Root cause analysis: at 10^18, y=19M → pi table = 76MB > L3 (36MB)
- Every S2_easy pi[xpq] lookup is a DRAM miss (100+ cycles vs 15 cycles L3 hit)
- Fix: cap y at 9M → pi table = 36MB, fits L3
- Tested alternatives that FAILED:
  - Segment cap (32M): made it much worse, didn't address pi table issue
  - Reciprocal division in S2_easy: no improvement (bottleneck is memory, not division)
  - Segmented pi-table processing: 29% slower due to rayon overhead from multiple passes
- Y cap shifts work from S2_easy to S2_hard, balancing concurrent tasks
- Results: 1 Quintillion 206.4s → 172.5s (16% faster, now 10% faster than V4!)
- V5 now faster than V4 at ALL measured scales from 100T to 1 Quintillion

### V5 Opt 5: Software prefetch for pi-table lookups (16% at 100Q)
- Added _mm_prefetch intrinsic with PREFETCH_DIST=8 in S2_easy inner loop
- Prefetches pi[xpq] for iteration l-8 to hide DRAM latency
- Also adds reciprocal multiplication (fast_div_easy) to S2_easy
- Tested DIST=1, 4, 8, 32. DIST=8 gave best results at 100Q.
- DIST=32 triggered Windows Application Control block (binary flagged as suspicious)
- Results: 100Q 24.81s → 21.00s (16% faster, 1.70× faster than V4!)
- No improvement at 1 Quintillion (pi table entirely in DRAM, prefetch distance too short)
- V5 performance summary (best results):
  - 100T: 0.092s (V4: 0.112s) = 1.22× faster
  - 1Q: 0.534s (V4: 0.770s) = 1.44× faster
  - 10Q: 3.41s (V4: 5.64s) = 1.65× faster
  - 100Q: 21.00s (V4: 35.80s) = 1.70× faster
  - 1 Quintillion: 172.4s (V4: 192.0s) = 1.11× faster

### V6 Implementation: Enhanced DR with Segmented Pi Table

**Concept**: Gourdon algorithm's key innovation — process pi table in L2-sized segments 
instead of requiring full table in L3. This eliminates the y-cap constraint from V5.

**The Problem**: V5 caps y at 9M so pi table (36MB) fits in L3. This doubles z=x/y, 
increasing S2_hard work. At 10^18: y should be 19M (pi=76MB) but is capped to 9M.

**V6 Solution**: Two-strategy adaptive dispatch:
1. Pi table <= L3 (36MB): V5's direct approach (parallel over b, prefetch)
2. Pi table > L3: Segmented approach — divide pi[0..y] into 512K-entry segments (2MB, L2-fit)
   - Parallel over segments via rayon
   - Each segment: iterate over valid b values, find l range where x/(p*q) falls in segment
   - All pi lookups hit L2 cache (~5ns vs DRAM 100ns)
   - Narrowed b-range per segment: max_b_seg = pi(sqrt(x/seg_low))

**Implementation Details**:
- compute_s2_easy_direct: exact copy of V5's S2_easy with prefetch
- compute_s2_easy_segmented: new segmented approach
- compute_s2_easy: dispatcher based on pi_table_bytes vs l3_size
- count_primes: NO y-cap (removed the 9M limit)
- S2_hard: identical to V5 (benefits from smaller z with larger y)

**Results (V6 Opt 0)**:
- Small scales (<=100Q): ~same as V5 (direct path used)
- 1 Quintillion: 113.4s vs V5 172.4s = **1.52x faster** (segmented path)
- Max i64: 547.3s (first measurement, V4 was 939.2s = **1.72x faster**)
- Key: uncapped y=19M halves z, reducing S2_hard work ~50%

**Key Insight**: Segmented pi processing eliminates the fundamental tension between 
S2_easy cache efficiency and S2_hard work volume. V5 had to compromise (cap y), 
V6 doesn't.

---

### V6 Opt 1 — Alpha Tuning + Segment Size (committed 12303b4)

**Alpha sweep**: Tested 12-50 at 1Q. Alpha=23 optimal (was 19 from V5).
Higher alpha = larger y = smaller z = fewer S2_hard segments.
With segmented S2_easy, larger y costs nothing in S2_easy.

**Segment size sweep**: 32K-2M entries. 128K (512KB) optimal.
Smaller segments complete S2_easy faster, freeing rayon threads for S2_hard.
S2_easy: 98→51s, S2_hard: 98→81s. Total: 99→82.6s.

**Combined**: 82.6s at 1Q (**27% faster** than Opt 0), 502.4s at Max i64.

---

### V6 S2_hard Exhaustive Analysis (no improvement found)

Tried EVERYTHING on S2_hard bottleneck (81s at 1Q):
- S2_hard segment size cap (L2-fitting): WORSE (88.7s). Larger segments amortize overhead.
- nchunks tuning: 3-24x multiplier. 6-12x all equivalent. Current 6x is optimal.
- target_segs tuning: 8-128x. All equivalent. No improvement.
- BitSieve hierarchical block counters: 57% overhead on hot crossing loop outweighs count() savings.

**Root cause**: S2_hard dominated by Type 1 leaves (b ≤ π(√y) ≈ 648) — millions of m 
iterations per prime checking mu[m] and lpf[m]. This is fundamental to DR algorithm.
Further gains require algorithmic changes (Gourdon) or optimizing other components.

---

### V6 Opt 2 — Segmented P2 (committed)

**Problem**: Old ParallelPiSieve allocates full sieve up to z:
- 1Q: 2.7GB bitmap + 2.7GB prefix = 5.4GB, uses rayon (steals threads from S2)
- Max i64: ~19.6GB total

**Solution**: Segmented sieve with 1M segments, sequential (no rayon):
1. Sort all (index, x/p) pairs by x/p value
2. Sweep 1M-number segments, sieve each, resolve queries as we go
3. Running π count across segments, 2-4MB memory

**Bug hunt**: Two boundary bugs:
1. When xp_val = seg_low (even, at segment boundary): largest_odd = seg_low - 1 falls 
   below segment, code incorrectly used bit_idx=0, counted seg_low+1 if prime.
   Fix: check `largest_odd <= seg_low` → use running_pi directly.
2. When max_xp is even at segment boundary: odd_count=0 causes early exit.
   Fix: round sieve_end to (max_xp | 1) + 1.

**Results**: All 16 tests pass ✓
- 1 Quintillion: 82.6s → **61.6s** (1.34x faster)
- Max i64: 502.4s → **400.8s** (1.25x faster)
- Memory: 5.4GB → 2-4MB (1350x reduction!)

**Double benefit**: P2 faster directly + no thread contention with S2.
S2_hard dropped from 81.7s → 60.7s (gets all 24 threads from start).

---

### V6 Opt 3 — ValidM List + Alpha Re-tune (committed)

**Problem**: S2_hard Type 1 inner loop iterates ~23M m values per b, most rejected.
15 billion iterations total at 1Q at ~5ns each = ~75 core-seconds = bottleneck.

**Solution**: Precompute filtered list of valid m values:
- Only include m where mu[m]!=0 AND lpf[m] > primes[c+1] (minimum threshold)
- List has ~14% of all m ≤ y (2.7M entries at 1Q vs 23M)
- 8-byte struct: m(u32) + lpf(u16) + mu(i8) + pad → ~21MB, fits in L3
- Binary search (partition_point) to find m range per (b, segment)
- Iteration: 15B → 1.75B = **8.5× fewer**

**Alpha re-tune**: With cheaper Type 1, optimal alpha shifts:
- 1Q: alpha=21 (was 23) gives 52.0s 
- Max i64: alpha~28 remains similar

**Results**:
- 1Q: 61.6s → **52.0s** (1.18x faster, S2_easy and S2_hard now balanced at ~51s)
- Max i64: 400.8s → **368.1s** (1.09x faster)
- Cumulative from Opt 0: 1Q 113.4s → 52.0s = **2.18x faster**

---

### V6 Opt 4 — P2 Pre-sieve Masks (committed)

**What**: Word-aligned bitmasks for primes 3, 5, 7 in P2 segmented sieve.
Precompute p masks per prime (bit patterns for each alignment offset).
Apply 3 masks per word in one pass: 64x fewer ops for these primes.

First segment: restore bits for primes 3, 5, 7 after mask application.

**Results**: P2 26-35% faster across all scales:
- 1Q: P2 32.3s → 23.0s, wall unchanged (S2_hard bottleneck)
- Max i64: P2 156.4s → 97.8s → wall 368.1s → **350.2s** (freed CPU time helps S2)
- Small scales 34% faster (100T: 0.33s → 0.22s, 1Q: 0.81s → 0.52s)

---

## V7 Gourdon's Algorithm — Optimization Campaign

### V7 Base — Gourdon's Algorithm (committed a7ac1a4)

**Algorithm**: π(x) = AC - B + D + Φ₀ + Σ (Gourdon 2001)

Key differences from V6 (Deleglise-Rivat):
- Two independent alpha parameters: α_y (for y) and α_z (for z)
- x* = max(x^(1/4), ⌈x/y²⌉) — tighter bounds than V6's √y
- D (hard leaves) processes fewer leaves than S2_hard
- BigPiTable: O(1) π(n) lookups via parallel sieve + prefix sums
- Concurrent B/AC/D via thread::scope

Five bugs fixed during development:
1. mu[p] computation: iterate from p (not 2p) in sieve
2. C2 lower bound: max(k, pi_root3_xy, pi_sqrtz) + 1
3. Pi table size: extended to max(z, max_a_prime)
4. cross_off_sieve: remove p² optimization (wrong for phi sieve)
5. Parallel D cur_max_b: match serial formula (no look-ahead)

**Results vs V6**:
- 1T: 0.008s (V6: 0.013s, 1.7x faster)
- 100Q: 7.05s (V6: 14.4s, 2.0x faster)
- 1 Quintillion: 32.4s (V6: 51.8s, 1.6x faster)
- Max i64: 202.1s (V6: 342.5s, 1.7x faster)

**Profile (V7 base)**:
| Component | 100Q | 1 Quint | Max i64 |
|-----------|------|---------|---------|
| Tables | 0.25s | 0.91s | 2.72s |
| B | 6.83s | 31.9s | 183.6s |
| AC | 0.89s | 9.84s | 184.8s |
| D | 6.28s | 28.8s | 197.9s |
| Wall | 7.1s | 32.9s | 200.9s |

B was the bottleneck (serial segmented sieve, single-threaded).

---

### V7 Opt 1 — Parallel B via BigPiTable (committed 59b7a51)

**Problem**: B used a serial segmented sieve (~150 lines) to compute Σ π(x/p) for
primes y < p ≤ √x. At 1Q, B = 31.9s single-threaded while D+AC took ~30s with 24 threads.

**Solution**: Replace serial sieve with BigPiTable-based approach:
- Build BigPiTable covering [0, x/smallest_prime] using parallel segmented sieve
- O(1) π(x/p) lookups for all primes via par_iter
- Run B concurrently with D+AC via thread::scope (all share rayon pool)
- Fix u32 overflow: pi(261B) > u32::MAX at Max i64 → use u64 prefix sums

Memory: ~6.9GB at 1Q, ~24.2GB at Max i64 (fits 96GB)

**Results**:
- 1Q: 32.9s → 29.0s (12% faster, B now hidden behind D)
- Max i64: ~same wall time (B and D both ~200s concurrent)
- Code reduction: 150 lines → 20 lines

---

### V7 Opt 2 — ValidM List for D Type 1 (committed 1345c29)

**Problem**: D Type 1 inner loop iterates ALL m in [min_m, max_m], checking
mu[m]!=0 && lpf[m]>prime && y_smooth[m]. Most rejected. ~8.5x more iterations than needed.

**Solution**: Port from V6 — precompute filtered list of valid m values:
- Only squarefree m with lpf > primes[c+1] and all factors ≤ y (y-smooth)
- Binary search (partition_point) for m range per (b, segment)
- 8-byte packed structs, ~21MB fits L3

**Results**:
- 100Q: 6.68s → 2.76s (**2.4x faster**)
- 1Q: 29.0s → 16.4s (**1.8x faster**)
- Max i64: ~200s (unchanged, D no longer bottleneck)

---

### V7 Opt 3 — Alpha Lookup Table (committed 6fffb1e)

**Problem**: Walisch's polynomial alpha curve was tuned for pre-ValidM cost profile.
After Opt 1+2 changed the component cost ratios, alpha needed re-tuning.

**Solution**: Replace polynomial alpha with piecewise-linear interpolation table.
Nine data points from logx=20 (x~5e8) to logx=43.7 (Max i64), with separate
alpha_y and alpha_z values at each point. Linear interpolation between points.

Table:
| logx | alpha_y | alpha_z | ~x |
|------|---------|---------|-----|
| 20.0 | 2.0 | 1.5 | 5e8 |
| 23.0 | 3.0 | 1.5 | 1e10 |
| 25.3 | 4.0 | 2.0 | 1e11 |
| 30.0 | 6.0 | 2.0 | 1e13 |
| 34.5 | 8.0 | 2.0 | 1e15 |
| 36.8 | 10.0 | 2.0 | 1e16 |
| 39.1 | 14.0 | 2.5 | 1e17 |
| 41.4 | 15.0 | 3.5 | 1e18 |
| 43.7 | 19.0 | 4.5 | Max i64 |

**Results — MASSIVE improvement**:
- 100Q: 2.74s (Opt 2: 2.76s, same)
- 1Q: 10.8s (Opt 2: 16.4s, **1.52x faster**)
- Max i64: 95.4s (Opt 2: ~200s, **2.1x faster**)

Cumulative V7 vs V6:
- 100Q: 2.74s vs 14.4s (**5.3x faster**)
- 1Q: 10.8s vs 51.8s (**4.8x faster**)
- Max i64: 95.4s vs 342.5s (**3.6x faster**)

**Profile after Opt 3** (all three concurrent):
| Component | 100Q | 1 Quint | Max i64 |
|-----------|------|---------|---------|
| Tables | 0.03s | 0.09s | 0.24s |
| B | 2.24s | 7.46s | 97.7s |
| AC | 1.32s | 8.03s | 87.7s |
| D | 2.08s | 9.37s | 87.7s |
| Wall | 2.73s | 11.4s | 104.8s |

At Max i64: B=97.7s is the bottleneck. AC=87.7s and D=87.7s perfectly balanced.
At 1Q: D=9.37s slight bottleneck. All three relatively close.

Key insight: The alpha table dramatically improved Max i64 by using α_y=19, α_z=4.5
(was ~alpha_y=15, alpha_z=3.5). Higher α_y pushes more work from D→AC (cheaper).
Higher α_z increases z, allowing more leaves to be classified as "easy" (AC) vs "hard" (D).

---

### Next Optimization Targets (post-Opt 3)

**Bottleneck analysis at Max i64**:
- B = 97.7s — builds 24GB BigPiTable covering [0, 232B], then parallel lookups
  - BigPiTable construction is the cost (parallel sieve of 232B numbers)
  - Could optimize sieve construction: pre-sieve masks, larger segments
- AC = 87.7s — segmented pi lookups using BigPiTable  
  - Could benefit from better clustering, cache-aware segment ordering
- D = 87.7s — parallel sieve-based hard leaves with ValidM
  - Already well-optimized, but could try compressed FactorTable, inline cross-off

**Remaining optimization ideas**:
1. B optimization: pre-sieve masks for BigPiTable construction (3, 5, 7)
2. Compressed FactorTable: pack mu/lpf/y_smooth into 2 bytes (halve memory, better cache)
3. AC segmented pi: V6-style L2-segmented processing for AC's pi lookups
4. Load-balanced D: work-stealing for uneven chunk sizes
5. Extended pre-sieve: add primes 17, 19 to pre-sieve template
6. Inline cross-off: 4x-unrolled cross-off with next[b] tracking in parallel D
7. Segment size tuning: sweep segment sizes for D and B
8. Alpha fine-tuning: sweep individual table entries for local optima


---

### V7 Opt 4 — Pre-sieve Masks + Alpha Table Fix (committed)

**Pre-sieve**: Word-level bitmask clearing for primes 3,5,7 in BigPiTable construction.
64x fewer operations for these three primes per segment. B at Max i64: 97.7s -> 87.0s (11%).
D also benefits from reduced rayon contention: 87.7s -> 75.4s.

**Alpha cliff discovery**: Exhaustive az sweep revealed a DRAMATIC performance cliff at az=4.5:
- az=4.4: 97.1s (B=88.3, AC=88.9, D=77.0)
- az=4.5: 59.4s (B=39.4, AC=53.5, D=50.4) — 37% FASTER from 0.1 change!

Root cause: concurrent thread contention. At az>=4.5, D and AC finish fast enough that B
gets exclusive rayon pool for its final BigPiTable construction. Below 4.5, all three compete
for threads for their full duration.

**The bug**: alpha table last entry at logx=43.7, but Max i64 logx=43.68. Interpolation gave
az=4.49 (just below cliff!). Fixed to logx=43.6 so Max i64 uses exact (19.0, 4.5).

**Results**: Max i64: 94.7s -> 62.2s (**34% faster**). 1Q unchanged at ~11s.

**Cumulative V7 Opt 4 vs V6**: Max i64 342.5s -> 62.2s = **5.5x faster**.

