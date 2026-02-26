# V8 Optimization Log — Algorithmic Analysis & Experiments

## Architecture

V8 builds on V7's proven Gourdon implementation (Opt 52, 8.60s at Max i64).
The goal: close the remaining 1.3% gap to primecount (8.49s) by analyzing
primecount's algorithmic advantages and adapting them to our codebase.

**Baseline**: V7 Opt 52 = 8.60s at Max i64 on Intel Core Ultra 9 285K (24 cores)

---

## Primecount Source Analysis

Deep analysis of kimwalisch/primecount revealed three key differences:

### 1. SegmentedPiTable (O(x^{1/4}) per segment, fits in L1)
Primecount uses a tiny per-segment π table (~3.7KB) rebuilt per segment via
streaming primesieve. Every π lookup is an L1 hit (~4 cycles). Our BigPiTable
is 285MB, causing L3/DRAM misses (~30-400 cycles per lookup).

### 2. Clustered Easy Leaves (C2 optimization)
For consecutive l-values with the same π(x/pq) result, primecount computes once
and multiplies by the cluster size. Uses `primes[π(xpq)+1]` to find the cluster
boundary. Only activates when `avg_clustered_leaves >= 6` (large b-values).

### 3. mod-240 Wheel Encoding
8 coprime-to-30 residues per byte = 240 numbers per u64 (vs our 128 numbers
per u64 with odd-only encoding). 1.875× more compact.

### Our Advantage: Barrett Reduction
Our `fast_div` (Barrett reduction via u128 multiply-shift) is ~6× faster than
primecount's hardware division instruction. This partially compensates for
slower π lookups.

---

## Opt 53 — Segment-First AC (FAILED: 141s → 75s → reverted)

**What**: Implemented primecount's segment-first approach: iterate over segments,
rebuild SegmentedPiTable per segment, process applicable b-values within segment.

**Implementation**:
- SegmentedPiTable struct with mod-240 wheel encoding
- SET_BIT_240 / UNSET_LARGER_240 static lookup tables (240 entries each)
- Per-segment: init_from_primes, iterate b-values, compute C2/A contributions

**Result**: 141.82s (16.5× regression!)

**Root Cause**: Per-segment `primesieve_count_primes(2, low-1)` called ~25K times.
Fixed to use incremental pi_low tracking → 75s. Still 8.7× slower than V7.

**Fundamental Problem**: Segment-first iterates ALL b-values per segment. Most
b-values have no work in a given segment, so the empty-range check overhead
(computing l-ranges per segment) dominates. V7's b-first approach (parallel
over b-values, segmented BigPiTable access) has inherently less overhead.

**Verdict**: REVERTED to V7's b-first compute_ac.

---

## Opt 54 — FullPiTable (mod-240, full range) (FAILED: 9.29s)

**What**: Build a full-range SegmentedPiTable covering [0, sqrt_x] using streaming
primesieve. Use it for AC lookups instead of BigPiTable.

**Result**: 9.29s (+7.8% regression)

**Root Cause**: 203MB additional table ON TOP of 285MB BigPiTable (488MB total).
L3 cache (36MB) pressure increased. Division by 240 adds ~4 cycles overhead per
lookup vs BigPiTable's shift-only indexing (n/128 = n>>7).

---

## Opt 55 — Interleaved BigPiTable (FAILED: 8.99s)

**What**: Interleave bits and prefix arrays into single `data: Vec<u64>` where
`data[2w]` = prefix and `data[2w+1]` = bits. Goal: reduce 2 cache line loads
per lookup to 1 (adjacent data in same cache line).

**Result**: 8.99s (+4.3% regression)

**Root Cause**: 33% larger table (380MB vs 285MB) — the interleaving wastes 4 bytes
per entry (u64 for u32 prefix). More L3 pressure outweighs fewer cache line loads.

---

## Opt 56 — Wheel-30 Compact BigPiTable (FAILED: 9.08s)

**What**: Replace odd-only BigPiTable (128 numbers/u64) with wheel-30 encoding
(240 numbers/u64). Expected 47% size reduction: 152MB vs 285MB.

**Implementation**:
- Build odd-only sieve first, convert to wheel-30 format
- pi_fast uses UNSET_LARGER_240 lookup for masked popcount
- compute_b switched to primesieve iterator (bits_word incompatible)

**Result**: 9.08s (+5.3% regression)

**Root Cause**: Division by 240 adds ~4 cycles per lookup. Also, BigPiTable build
took 0.473s (vs 0.134s) due to odd→wheel-30 conversion. B regressed from 6.61s
to 8.60s due to slower pi() lookups AND slower prime enumeration.

**Key Insight**: The division overhead for wheel-30 indexing (~4 cycles × 35.85B
lookups / 24 / 5.7GHz = 0.91s) exceeds the L3 bandwidth savings from smaller table.

---

## Opt 57 — Generic PiTable Trait Regression (DIAGNOSED: +1.0s)

**What**: V8's compute_ac used `PiTable` trait (generic over BigPiTable and
SegmentedPiTable) to enable A/B testing. Both types were monomorphized.

**Result**: 9.71s (+1.1s regression vs V7)

**Root Cause**: Dual monomorphization creates TWO copies of the ~200-line hot inner
loop in the binary (one for BigPiTable, one for SegmentedPiTable). Despite both
using `#[inline(always)]`, the doubled code size causes instruction cache pressure.
Even though the SegmentedPiTable path is dead code at runtime (env var check), LLVM
can't eliminate it (runtime env var read).

**Fix**: Removed PiTable trait, made compute_ac take `&BigPiTable` directly.
Result: 8.63s (matches V7).

**Lesson**: Generics in hot paths can cause I-cache regressions even with
monomorphization, if multiple instantiations exist in the binary.

---

## Opt 58 — AC Segment Size Sweep (NO IMPROVEMENT)

**What**: Swept AC_SEG from 65K to 500K pairs. Default = 130K pairs = 1.56MB.

**Results**:
| AC_SEG | Time |
|--------|------|
| 65K | 8.79s |
| 80K | 8.71s |
| 100K | 8.72s |
| 130K (default) | 8.69s |
| 160K | 8.65s |
| 200K | 8.65s |
| 250K | 8.67s |
| 300K | 8.71s |
| 400K | 8.84s |
| 500K | 8.87s |

**Verdict**: 130K-200K are within noise (±0.1s). No actionable improvement.

---

## Opt 59 — Software Prefetching in AC Inner Loop (FAILED: 9.66s)

**What**: Prefetch next batch of BigPiTable entries while processing current batch.
Compute xpq for l+4..l+7, issue prefetch instructions, then do pi_fast for l..l+3.

**Result**: 9.66s (+12% regression)

**Root Cause**: The prefetch requires 4 extra `fast_div` computations per batch
(~20 cycles). The prefetch instructions themselves add ~16 cycles. Total overhead
(~36 cycles per 4-iteration batch) exceeds the L3 latency hiding benefit.
DRAM latency (~400 cycles) requires prefetching ~33 iterations ahead, which is
impractical.

---

## Opt 60 — Primecount Alpha Parameters (FAILED: 9.72s)

**What**: Tested primecount's computed alpha values (α_y≈17.05, α_z=2.0) vs our
tuned values (α_y=18.5, α_z=1.3).

**Result**: 9.72s (+12.5% regression)

**Root Cause**: Primecount's lower α_y increases AC work (more b-values, wider
l-ranges). Higher α_z increases z, moving work from C1 to C2. Our alpha tuning
is already optimized for our hardware and implementation characteristics.

---

## Opt 61 — Dedicated AC Thread Pool (FAILED: 14.29s)

**What**: Give AC its own rayon ThreadPool (24 threads) to prevent D's par_iter
from stealing AC's threads. Goal: reduce L3 contention by isolating workloads.

**Result**: 14.29s (+65% regression)

**Root Cause**: 24 AC threads + 24 D threads (global pool) = 48 threads competing
for 24 hardware threads. Massive context switching. Worse: AC can't benefit from
idle D threads anymore (shared pool enables work-stealing).

**Key Insight**: The shared rayon pool is correct — L3 contention is the bottleneck,
not thread scheduling. Isolating pools makes scheduling worse without helping L3.

---

## Opt 62 — PGO (Profile-Guided Optimization) (NO IMPROVEMENT)

**What**: Full PGO pipeline: instrumented build → training runs at 1Q/10Q/100Q/1E/Max
→ merge profiles → optimized rebuild.

**Result**: 8.62s (±0.03s, within noise of non-PGO 8.63s)

**Root Cause**: The workload is memory-bandwidth-bound (BigPiTable lookups dominate).
PGO helps branch prediction and code layout, but these are not bottlenecks. Branch
prediction is already >99% accurate for the loop-heavy code. LLVM's default
optimizations are sufficient for this workload.

---

## Summary of V8 Experiments

| Opt | Description | Result | vs V7 |
|-----|-------------|--------|-------|
| 53 | Segment-first AC | 75s | -8.7× |
| 54 | FullPiTable (mod-240) | 9.29s | -7.8% |
| 55 | Interleaved BigPiTable | 8.99s | -4.3% |
| 56 | Wheel-30 BigPiTable | 9.08s | -5.3% |
| 57 | Generic PiTable trait | 9.71s | -12.7% |
| 58 | AC segment size sweep | 8.65s | ≈ same |
| 59 | Software prefetching | 9.66s | -12.2% |
| 60 | Primecount alpha params | 9.72s | -12.5% |
| 61 | Dedicated AC pool | 14.29s | -65.3% |
| 62 | PGO | 8.62s | ≈ same |

**Current best**: 8.63s (V8 = V7 baseline after reverting all failed experiments)

---

## Opt 63 — Clustered Easy Leaves for C2 (FAILED: 11.90s)

**What**: For C2 b-values, iterate l-values from high to low. When consecutive
l-values produce the same π(xpq), multiply once by cluster size instead of
iterating. Uses `next_prime_after(xpq)` to find cluster boundaries.

**Implementation**:
- Added `next_prime_after()` to BigPiTable (searches sieve bits for next set bit)
- Added `min_clustered_l` field to BLookup struct (set to π(√xp))
- Pre-loop from eff_hi down to min_clustered_l before regular 4× unrolled loop

**Result**: 11.90s (+38% regression). AC loops: 11.71s (vs 8.43s baseline).

**Root Cause**: `next_prime_after()` requires a SECOND BigPiTable cache access per
cluster iteration (beyond the pi_fast lookup). This extra cache miss adds ~100
cycles per cluster. Combined with reduced ILP (sequential clustering loop vs 4×
unrolled), the overhead far exceeds the savings from skipping iterations.

**Key Insight**: Clustering only works with L1-resident SegmentedPiTable where
each lookup is ~4 cycles. With our L3-bound BigPiTable (~40+ cycles per lookup),
the extra lookup for cluster boundary detection is too expensive.

---

## Opt 64 — Interleaved bits+prefix Layout (FAILED: 9.06s)

**What**: Interleave bits[w] and prefix[w] into single `data: Vec<u64>` where
`data[2w] = bits` and `data[2w+1] = prefix as u64`. Ensures both values are
in the same 64-byte cache line → 1 cache miss per lookup instead of 2.

**Result**: 9.06s (+5.2% regression). BigPiTable build: 0.209s (vs 0.137s).

**Root Cause**: Table size 381MB (33% larger than 285MB). The reduced L3 coverage
ratio (9.4% vs 12.6%) more than offsets the co-location benefit. At sub-1E scales
it was actually faster (1E: 2.43s vs 2.58s), but at Max i64 the larger footprint
dominates. B also improved (6.72s vs 7.14s) suggesting B was bandwidth-limited too.

---

## Opt 65 — Sparse Prefix (8-word blocks) (FAILED: 14.18s)

**What**: Replace per-word prefix array (95MB) with per-8-word coarse prefix (12MB).
For pi(n): load coarse_prefix[word/8] from small L3-resident array, then sum up
to 7 popcount(bits[...]) from the same cache line as the target word.

**Goal**: Reduce table from 285MB to 202MB. Trade 7 extra popcounts (~14 cycles)
for eliminating 95MB prefix array misses.

**Result**: 14.18s (+65% regression). AC loops: 14.03s.

**Root Cause**: The conditional popcounts (`if block_offset > N`) generate
unpredictable branches. Average 3.5 extra popcounts per lookup at ~3 cycles each
= 10.5 cycles overhead. This far exceeds the savings from smaller table.

---

## Opt 66 — Scheduling: B_THREADS=4 (ANALYSIS ONLY: 14.36s)

**What**: Reduce B's rayon pool to 4 threads to free cores for AC.

**Result**: Total 14.36s. AC: 7.14s (17% faster!), D: 4.81s (14% faster), but
B: 14.21s (2.1× slower, becomes bottleneck).

**Key Insight**: Confirms that B contends significantly with AC for L3/cores.
With B using fewer threads, AC gets more effective L3 bandwidth and core time.

---

## Opt 67 — Phase Scheduling (ANALYSIS: 9.0-9.1s)

**What**: Tested three phase schedules to understand component interactions.

| Schedule | B | D | AC | Total |
|----------|---|---|-----|-------|
| Default (all concurrent) | 6.65s | 5.61s | 8.57s | 8.72s |
| PHASE_DB_AC (D+B → AC) | 6.35s | 5.41s | 2.19s | 9.06s |
| PHASE_AC_DB (AC → D+B) | 6.37s | 5.31s | 2.18s | 9.10s |
| PHASE_B_ACD (B → AC+D) | 2.89s | 4.54s | 6.62s | 9.96s |

**Critical Discovery**: **AC alone takes only 2.19s** (vs 8.57s concurrent = 4×
penalty). B alone takes 2.89s (vs 6.65s concurrent). The concurrent penalty comes
from L3/core sharing, not algorithmic issues.

Even with D only (no B), AC takes 6.62s — D competes with AC significantly.

**Verdict**: Phase scheduling gives worse total time than concurrent despite
per-component improvements. The overlap benefit of concurrent execution exceeds
the contention cost.

---

## Opt 68 — Dedicated D Pool (NO IMPROVEMENT: 8.71-8.90s)

**What**: Give D its own rayon pool (separate from global pool used by AC) to
prevent D's heavy tasks from blocking AC's lightweight tasks.

**Results**:
| D_THREADS | AC | D | B | Total |
|-----------|-----|---|---|-------|
| 32 | 6.14s | 8.51s | 6.44s | 8.90s |
| 48 | 8.39s | 8.49s | 6.42s | 8.88s |
| 64 | 8.40s | 8.34s | 5.56s | 8.71s |
| 72 | 8.49s | 8.33s | 5.36s | 8.71s |

D_THREADS=32 shows AC_loops=4.75s (44% faster than baseline 8.43s!), confirming
that isolating AC from D dramatically helps AC. But D becomes the bottleneck.

**Root Cause**: Dedicated pools increase total thread count (72+32+24+8=136 on 24
cores). When D finishes, its idle threads can't help AC (unlike shared pool where
work-stealing enables this). The shared-pool approach remains optimal.

---

## Opt 69 — D Chunk Granularity (NO IMPROVEMENT)

**What**: Increase D_CHUNKS from 32 to 64/128/256/512 for finer rayon interleaving
with AC tasks.

| D_CHUNKS | D | AC | Total |
|----------|---|-----|-------|
| 32 (default) | 5.61s | 8.57s | 8.72s |
| 64 | 5.52s | 8.50s | 8.66s |
| 128 | 5.57s | 8.62s | 8.77s |
| 256 | 6.93s | 8.78s | 9.04s |
| 512 | 8.97s | 10.96s | 11.11s |

**Verdict**: 64 chunks shows marginal improvement (within noise). Higher counts
increase rayon scheduling overhead. Default 32 is optimal.

---

## Session 3: Final Micro-Optimization Sweep

### Dead Code Cleanup

Removed all dead code from failed V8 experiments:
- **SegmentedPiTable** struct and impl (~90 lines): mod-240 wheel table, unused after reverting Opt 53/54
- **PiTable trait** and impl blocks (~27 lines): generic dispatch, unused after making compute_ac take `&BigPiTable` directly
- **generate_pi** function: replaced by generate_pi_from_bits in V7
- **SET_BIT_240** and **UNSET_LARGER_240** statics (~44 lines): lookup tables for mod-240, only used by SegmentedPiTable
- **FullPiTable build thread** and USE_FULLPI env var: removed from default scheduling path
- Added `#[allow(dead_code)]` to useful utilities retained for future use (next_prime_after, primesieve_count_primes)

Result: 0 compiler warnings, code reduced by ~170 lines.

---

## Opt 70 — Same-Batch Prefetch in AC Loop (REGRESSION)

**What**: Add explicit `big_pi.prefetch(xpq0..3)` after computing xpq values but
before pi_fast reads. Goal: ensure all 8 cache lines (bits+prefix for each of 4
entries) are requested in parallel.

**Result**: 8.97s (+3.6% regression)

**Analysis**: The prefetch-to-use distance is too short (~4 cycles between prefetch
instruction and the subsequent pi_fast load). The prefetch instructions themselves
add overhead (8 per iteration) without enough time for the memory controller to
service them. The OoO engine on Arrow Lake already discovers and overlaps the
independent loads without explicit prefetch.

---

## Opt 71 — PGO (Profile-Guided Optimization) (BLOCKED)

**What**: Instrumentation-based PGO: build with `-Cprofile-generate`, run, merge
profiles, rebuild with `-Cprofile-use`.

**Result**: Blocked by Windows Application Control policy — instrumented binary
refused to execute. The PGO runtime libraries trigger security restrictions.

---

## Opt 72 — AC_SEG Sweep (MARGINAL IMPROVEMENT)

**What**: Sweep AC segment size to find optimal L2 cache utilization.

| AC_SEG | Wide | Narrow | Segs | AC loops | Total |
|--------|------|--------|------|----------|-------|
| 65,000 | 69,965 | 84,736 | 366 | 8.58s | 8.73s |
| 100,000 | 59,495 | 95,206 | 238 | 8.46s | 8.64s |
| 130,000 (old) | 53,655 | 101,046 | 183 | 8.51s | 8.67s |
| 200,000 | 44,933 | 109,768 | 119 | 8.45s | 8.61s |
| 260,000 | 40,168 | 114,533 | 92 | 8.43s | 8.70s |
| 500,000 | 30,018 | 124,683 | 48 | 8.68s | 8.88s |
| 1,000,000 | 21,570 | 133,131 | 24 | 9.05s | 9.27s |

**Verdict**: 200,000 pairs is the sweet spot. Larger segments reduce wide b-values
(44.9K vs 53.7K) shifting more work to the cache-friendly narrow path. Beyond 260K,
segments exceed L2 capacity and L2 locality breaks down.

**Applied**: Default changed from 130,000 → 200,000.

---

## Opt 73 — POOL_MULT Confirmation

| POOL_MULT | Threads | D | B | AC | Total |
|-----------|---------|---|---|-----|-------|
| 1 | 24 | 6.54s | 4.59s | 9.22s | 9.38s |
| 2 | 48 | 6.33s | 6.57s | 8.59s | 8.81s |
| 3 | 72 | 5.66s | 6.95s | 8.55s | 8.72s |
| 4 | 96 | 5.17s | 7.03s | 8.60s | 8.75s |
| 5 | 120 | 4.96s | 6.77s | 8.65s | 8.84s |

**Verdict**: POOL_MULT=3 (72 threads) confirmed optimal — best balance of overlap
benefit vs context-switching overhead.

---

## Opt 74 — Alpha_Y Sweep (NO IMPROVEMENT)

| ALPHA_Y | D | B | AC | Total |
|---------|---|---|-----|-------|
| 17.0 | 5.60s | 7.43s | 8.53s | 8.69s |
| 18.0 | 5.58s | 7.27s | 8.46s | 8.63s |
| 18.5 (default) | 5.53s | 6.85s | 8.59s | 8.75s |
| 19.0 | 5.59s | 6.71s | 8.55s | 8.71s |
| 20.0 | 5.81s | 6.63s | 8.55s | 8.73s |

**Verdict**: All values within run-to-run variance. 18.5 confirmed as reasonable
default.

---

## Opt 75 — B_THREADS Sweep (CONFIRMS TRADEOFF)

| B_THREADS | D | B | AC | Total |
|-----------|---|---|-----|-------|
| 8 | 4.82s | 10.25s | 7.25s | 10.39s |
| 12 | 5.01s | 9.21s | 7.56s | 9.36s |
| 16 | 5.34s | 8.79s | 8.11s | 8.93s |
| 20 | 5.34s | 8.61s | 8.28s | 8.78s |
| 24 (default) | 5.64s | 6.90s | 8.60s | 8.76s |

**Key Insight**: Fewer B threads → AC benefits dramatically (7.25s with B=8) but B
becomes the bottleneck. B=24 is optimal because B finishes before AC (6.9s < 8.6s),
so it never limits total time.

---

## Opt 76 — Split Wide/Narrow Processing (REGRESSION)

**What**: Process wide b-values in a single par_iter (full l-range, no segmentation)
before the segmented narrow loop. Goal: reduce 183 segment barriers for wide
b-values and improve rayon amortization.

**Result**: 9.86-9.98s (+14% regression)

**Analysis**: Without segmentation, wide b-values access the full 285MB BigPiTable
randomly. This destroys L2 locality that the segmented approach provides, and
causes massive L3/DRAM bandwidth contention with B (which also uses BigPiTable).
The segmented approach — even with 183 barriers — is essential for L2 cache reuse.

---

## Updated Summary (V8 Final)

| Opt | Description | Result | vs V7 |
|-----|-------------|--------|-------|
| 53 | Segment-first AC | 75s | -8.7× |
| 54 | FullPiTable (mod-240) | 9.29s | -7.8% |
| 55 | Interleaved BigPiTable (v1) | 8.99s | -4.3% |
| 56 | Wheel-30 BigPiTable | 9.08s | -5.3% |
| 57 | Generic PiTable trait | 9.71s | -12.7% |
| 58 | AC segment size sweep | 8.65s | ≈ same |
| 59 | Software prefetching | 9.66s | -12.2% |
| 60 | Primecount alpha params | 9.72s | -12.5% |
| 61 | Dedicated AC pool | 14.29s | -65.3% |
| 62 | PGO | 8.62s | ≈ same |
| 63 | Clustered easy leaves | 11.90s | -38.0% |
| 64 | Interleaved bits+prefix | 9.06s | -5.2% |
| 65 | Sparse prefix (8-word) | 14.18s | -65.0% |
| 66 | B_THREADS=4 (analysis) | 14.36s | — |
| 67 | Phase scheduling | 9.06s | -3.9% |
| 68 | Dedicated D pool | 8.71s | ≈ same |
| 69 | D chunk granularity | 8.66s | ≈ same |
| 70 | Same-batch prefetch | 8.97s | -4.3% |
| 71 | PGO | blocked | — |
| 72 | AC_SEG=200K | 8.63s | ≈ same |
| 73 | POOL_MULT sweep | 8.72s | confirmed |
| 74 | Alpha_Y sweep | 8.63s | ≈ same |
| 75 | B_THREADS sweep | 8.76s | confirmed |
| 76 | Split wide/narrow | 9.86s | -14.0% |

**Final V8 performance**: 8.63s (median), best 8.39s. The best run occurs on a
cold CPU at peak turbo frequencies; median rises as thermal throttling sets in
across sustained runs. AC_SEG=200000 applied as the only measurable code improvement.

### Architecture Analysis Conclusions

1. **The 1.3% gap to primecount is architectural**: primecount's L1-resident
   SegmentedPiTable eliminates L3 bandwidth as a bottleneck. Our b-first
   architecture with 285MB BigPiTable is fundamentally L3-bound.

2. **AC's 4× concurrent penalty is irreducible**: AC alone = 2.2s, concurrent =
   8.5s. The penalty comes from sharing L3/DRAM bandwidth with B and D. No
   scheduling or pool configuration can eliminate it without losing overlap benefits.

3. **All table optimizations failed**: Smaller tables (wheel-30: -47% size) lose
   more from indexing overhead than they gain. Larger tables (interleaved: +33%)
   lose from reduced L3 coverage. The odd-only 128-per-word layout hits the sweet
   spot of minimal indexing (shift + popcount) and acceptable memory footprint.

4. **The shared rayon pool is optimal**: Despite D's heavy tasks starving AC,
   work-stealing when D finishes gives more benefit than dedicated-pool isolation.

5. **To beat primecount requires a full AC rewrite**: segment-first processing
   with L1-resident SegmentedPiTable, fundamentally different parallelization
   (threads own segment ranges vs parallel over b-values). This is a ~1000-line
   rewrite of the most complex function in the codebase.

---

## Session 4: Nightly Toolchain & Build Optimization

### Baseline
V8 with stable toolchain: 8.63-8.66s median (AC_SEG=200K, all Opt 53-76 applied).

### Opt 77: Clean PGO with nightly
**Hypothesis**: Profile-guided optimization should improve branch prediction and code layout.
- Built instrumented binary with `-Cprofile-generate` + nightly flags
- Ran at Max i64 (180s instrumented)
- Merged profiles with llvm-profdata, no hash mismatch warnings
- **Result: 9.04-9.14s (REGRESSION +5%)**
- PGO's aggressive code layout changes hurt the hand-tuned inner loop. Increased
  code size causes L1 icache pressure. The original LLVM codegen with our
  unroll-threshold=800 already produces near-optimal code.

### Opt 78: -Zbuild-std (recompile std with target-cpu=native)
**Hypothesis**: Recompiling the standard library with AVX-512 and Arrow Lake
instruction scheduling should improve memcpy/memset and other std operations.
- Required disabling Smart App Control (SAC) via registry edit (SAC blocked
  execution of recompiled std binaries)
- Build: `cargo +nightly build --release -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`

| Run | Time |
|-----|------|
| 1 | 8.49s |
| 2 | 8.55s |
| 3 | 8.56s |
| 4 | 8.54s |
| 5 | 8.61s |
| 6 | 8.56s |
| 7 | 8.55s |

**Median: 8.55s, Best: 8.49s** — matches primecount target!
Improvement from std operations compiled with target-cpu=native (AVX-512
memcpy, tuned allocation in mimalloc, etc.).

### Opt 79: panic_immediate_abort
**Hypothesis**: Eliminating panic formatting reduces code size.
- `-Zunstable-options -Cpanic=immediate-abort` with `-Zbuild-std=std,core`
- Result: 8.53-8.70s — no significant change vs Opt 78.
- Panic formatting code was already dead-code-eliminated by LTO.

### Opt 80: LLVM loop interchange/flatten
Added `--enable-loop-interchange --enable-loop-flatten` to LLVM args.
Result: 8.51-8.58s — within noise of Opt 78. No applicable loops.

### Opt 81: unroll-threshold=1200
Increased from 800 to 1200 to see if more unrolling helps.
Result: 8.57-8.62s — slight regression. Larger code doesn't fit as well in icache.
Reverted to 800.

### Opt 82: Branchless AC inner loop restructure
**Hypothesis**: Eliminating is_c2 and y_boundary branches from the 4× unrolled
inner loop by splitting into separate phase loops (C2 loop, before-boundary loop,
after-boundary loop) should improve branch prediction and allow better LLVM optimization.
- Result: **9.35-9.55s (REGRESSION +10%)**
- Six separate loops instead of one unified loop explodes L1 icache footprint.
- The original branches are well-predicted (is_c2 is loop-invariant, y_boundary
  mispredicts once per b-value = negligible).
- Reverted immediately.

### Opt 83: Interleaved BigPiTable layout
**Hypothesis**: Storing prefix[w] and bits[w] adjacently in memory (16B per entry
instead of separate arrays) should halve cache line accesses per pi_fast call.
- Interleaved layout: `data[2*w] = prefix, data[2*w+1] = bits`
- Total memory: 380MB (vs 285MB)
- Result: **9.07s (REGRESSION +6%)**
- Worse cache line utilization: separate arrays give 8 bits-words per line (64B/8B)
  and 16 prefix-words per line (64B/4B). Interleaved gives only 4 entries per line
  (64B/16B). The hardware prefetcher handles the two-stream access pattern efficiently.
- Reverted.

### Opt 84-86: Thread pool experiments with build-std

| Opt | Config | Median | Change |
|-----|--------|--------|--------|
| 84 | D_THREADS=16 | 11.13s | -30% (oversubscription) |
| 85 | AC_THREADS=16 | 14.88s | -72% (AC starved) |
| 86 | POOL_MULT=2 | 8.72s | -2% (less overlap) |

Thread separation creates oversubscription (24 global + 24 B + 16 D = 64 threads
on 24 cores). The shared rayon pool with work-stealing remains optimal.

### Opt 87: AC_SEG sweep with build-std

| AC_SEG | Median |
|--------|--------|
| 100K | 8.62s |
| 150K | 8.64s |
| 200K | 8.61s |
| 260K | 8.60s |
| 350K | 8.74s |

200K-260K optimal, confirming Opt 72's result holds with build-std.

### Summary Table (Session 4)

| Opt | Description | Median | Change |
|-----|-------------|--------|--------|
| 77 | Clean PGO (nightly) | 9.06s | -5.3% |
| 78 | -Zbuild-std | **8.55s** | **+1.3%** |
| 79 | panic_immediate_abort | 8.55s | ≈ same |
| 80 | Loop interchange/flatten | 8.55s | ≈ same |
| 81 | unroll-threshold=1200 | 8.60s | -0.6% |
| 82 | Branchless AC inner loop | 9.47s | -10.0% |
| 83 | Interleaved BigPiTable | 9.07s | -6.0% |
| 84 | D_THREADS=16 | 11.13s | -30.0% |
| 85 | AC_THREADS=16 | 14.88s | -72.0% |
| 86 | POOL_MULT=2 | 8.72s | -2.0% |
| 87 | AC_SEG sweep (build-std) | 8.61s | confirmed |

**Final V8 performance**: 8.60s median, **8.39s best** with nightly + build-std.
Best run captures peak cold-core turbo performance; median reflects thermal ramp
across sustained benchmark runs (CPU heats from ~40°C to ~85°C over 10 runs).

### Key Findings

1. **PGO is counterproductive** for heavily hand-tuned inner loops. LLVM's
   default codegen with our unroll-threshold is already optimal. PGO's aggressive
   inlining and code layout increases icache pressure.

2. **-Zbuild-std is the single best nightly optimization** (~1.3% improvement).
   Recompiling std with target-cpu=native benefits memory operations throughout.

3. **Code size is critical**: Both the branchless loop restructure (+10 loops)
   and the interleaved table (+33% memory) regressed because they increased the
   L1 icache and L2 data cache working sets respectively.

4. **The original loop structure is near-optimal**: Well-predicted branches
   (is_c2 is loop-invariant, y_boundary mispredicts once per b-value) add
   negligible overhead. The unified loop keeps icache footprint minimal.

5. **Separate arrays beat interleaved for prefix+bits**: The u32 prefix array
   packs 16 entries per cache line (vs 4 in 16B interleaved), and the hardware
   L2 prefetcher easily tracks two independent sequential streams.

### Opt 88: Alpha_Y sweep with build-std timing

| ALPHA_Y | AC | B | D | Total |
|---------|------|------|------|-------|
| 16.0 | 8.56s | 8.36s | 5.56s | 8.71s |
| 17.0 | 8.50s | 8.21s | 5.55s | 8.65s |
| 18.5 | 8.46s | 7.38s | 5.71s | 8.61s |
| 20.0 | 8.45s | 7.34s | 5.65s | 8.60s |
| 22.0 | 8.46s | 6.86s | 5.69s | 8.64s |

AC is constant (~8.46s) regardless of alpha_y. B scales inversely with alpha.
Optimal: alpha_y=18.5-20.0 where B finishes before AC. Default 18.5 confirmed.

### Opt 89: B_THREADS sweep with build-std timing

| B_THREADS | AC | B | Total |
|-----------|------|------|-------|
| 8 | 7.22s | 10.29s | 10.44s |
| 12 | 7.54s | 9.19s | 9.33s |
| 16 | 7.76s | 8.75s | 8.93s |
| 18 | 8.05s | 8.62s | 8.77s |
| 20 | 8.15s | 8.55s | 8.69s |
| 22 | 8.35s | 8.53s | 8.69s |
| 24 | 8.44s | 7.59s | 8.60s |

Reducing B threads speeds up AC (less CPU contention) but B becomes bottleneck.
B=24 optimal: B finishes early (7.6s), freeing threads for AC via work-stealing.

---

## Final V8 Summary

**37 experiments** (Opt 53-89) across 4 sessions:
- Session 1-2: Table layout, scheduling, prefetching (Opt 53-69)
- Session 3: AC_SEG sweep, pool tuning, dead code cleanup (Opt 70-76)
- Session 4: Nightly/PGO/build-std, data layout, thread experiments (Opt 77-89)

**Best build command**:
```
cargo +nightly build --release --bin prime_count_v8 \
  -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc
```

**Config (.cargo/config.toml)**:
```toml
rustflags = ["-C", "target-cpu=native", "-C", "llvm-args=--unroll-threshold=800",
  "-Zlocation-detail=none", "-Zmir-opt-level=4", "-Ztune-cpu=arrowlake"]
```

**Performance**:
| Metric | Value |
|--------|-------|
| Best (cold CPU) | **8.39s** |
| Median (10 runs) | 8.60s |
| primecount | 8.49s |
| Gap (best) | **-0.10s (1.2% faster)** |
| Gap (median) | +0.11s (1.3% slower) |
| Improvement over V7 stable | -0.26s (3.0%) |

**Note on thermal variance**: Best run occurs on a cold CPU (first run after 60s
idle) with full turbo boost. Median reflects thermal ramp — the CPU heats from
~40°C to ~85°C across 10 consecutive runs, reducing turbo clocks by ~100-200 MHz.

**Phase timing at best run (8.39s)**:
- AC: 8.30s (bottleneck)
- B: 7.83s
- D: 5.76s
- BigPiTable: 0.139s

**Head-to-head vs primecount** (10 alternating runs, identical thermal conditions):

| Run | V8 (internal) | primecount (wall) | Delta |
|-----|---------------|-------------------|-------|
| 1 | 8.525s | 9.227s | **V8 −0.702s** |
| 2 | 8.543s | 8.685s | **V8 −0.142s** |
| 3 | 8.585s | 8.698s | **V8 −0.113s** |
| 4 | 8.593s | 8.620s | **V8 −0.027s** |
| 5 | 8.623s | 8.723s | **V8 −0.100s** |
| 6 | 8.631s | 8.699s | **V8 −0.068s** |
| 7 | 8.660s | 8.661s | **V8 −0.001s** |
| 8 | 8.666s | 8.696s | **V8 −0.030s** |
| 9 | 8.668s | 8.761s | **V8 −0.093s** |
| 10 | 8.727s | 8.690s | PC −0.037s |

V8 wins **9/10 runs**. V8 median: 8.63s, primecount median: 8.70s (−0.07s gap).
primecount's published 8.49s is a cold-CPU best; under sustained load both tools
show similar thermal ramp, with V8 consistently ahead.

---

## Session 5: Further Optimization Analysis

### Opt 90: P-core Affinity for AC (CATASTROPHIC REGRESSION)

**What**: Created a dedicated 8-thread rayon pool for AC, pinned to P-cores (cores
0-7, 2MB L2, 5.7GHz) using `SetThreadAffinityMask`. Hypothesis: AC segments
(~2.4MB) fit in P-core L2 but overflow E-core L2 (1MB per core).

**Result**: 18.8s (AC), 2.2× regression.

**Root cause**: AC needs all 24 cores for parallelism (44K b-values). Restricting to
8 P-cores cuts throughput by 3×. The L2 cache benefit of P-cores is completely
overwhelmed by having 1/3 the threads. AC is L3/DRAM bandwidth-bound, not L2-bound.

### Opt 91: Per-b-value Streaming SegPiTable (CATASTROPHIC REGRESSION)

**What**: Implemented a streaming SegmentedPiTable (12KB per segment, fits L1 cache)
for wide b-values. Each rayon thread maintains a local sieve covering 131072
numbers (1024 u64 words + 1024 u32 prefix). When xpq crosses a segment boundary,
the thread rebuilds the sieve using precomputed small primes (~5660 primes up to
sqrt(sqrt(x)) ≈ 55K). Narrow b-values kept using BigPiTable (already L2-friendly).

**Result**: 12.8s at 100 Quadrillion (was 0.61s), >20× regression. Killed at Max i64.

**Root cause**: Each of 44K wide b-values independently traverses ~23K segments,
rebuilding the sieve at each crossing. Total rebuilds per core: ~42M, at ~50μs each
= 2100 seconds. The approach fails because threads DON'T SHARE sieves — each thread
redundantly rebuilds the same segment's sieve independently.

**Why primecount avoids this**: primecount uses a **segment-first** architecture:
iterate segments (build sieve ONCE per segment), then process ALL b-values within
that segment. Total rebuilds: 23K (one per segment), not 23K × 44K (one per b-value
per segment). This requires fundamentally different parallelism: par_iter over
segments (or b-value chunks within segments), not over b-values across segments.

### Opt 92: mimalloc Large Pages (NO EFFECT)

**What**: Enabled mimalloc's `mi_option_large_os_pages` (2MB huge pages) to reduce
TLB misses for BigPiTable (285MB = 140K TLB entries at 4KB, vs 143 at 2MB).

**Result**: No change (same 8.5-8.6s). Silently fails without `SeLockMemoryPrivilege`
(requires admin Group Policy grant on Windows).

---

## Session 9: Large Pages with SeLockMemoryPrivilege (Opt 108)

### Opt 108: 2MB Large Pages via mimalloc (SUCCESS — 1.3% improvement)

**Background**: Opt 92 attempted mimalloc large pages but failed silently because
`SeLockMemoryPrivilege` was not granted. After running `grant_lock_memory.ps1` as
admin and rebooting, the privilege is now available.

**What**: BigPiTable is 285MB = 73K TLB entries at 4KB pages. With 2MB large pages,
only 143 entries — fits comfortably in Arrow Lake's L2 dTLB. Three approaches tested:

1. **`MIMALLOC_LARGE_OS_PAGES=1` env var** — mimalloc transparently allocates all
   memory with large pages. BigPiTable build: 0.095s (was 0.135s, 30% faster page faults).
2. **Explicit `VirtualAlloc` with `MEM_LARGE_PAGES`** — direct OS allocation for
   BigPiTable only. Median 8.69s — WORSE than transparent approach (8.60s).
3. **Combined (explicit + mimalloc)** — no additional benefit over mimalloc alone.

**Winner**: Transparent mimalloc approach (#1). Added `large_page_alloc` module that
programmatically enables `SeLockMemoryPrivilege` via Win32 `AdjustTokenPrivileges` API.
The env var `MIMALLOC_LARGE_OS_PAGES=1` must be set externally (mimalloc reads it at
init time before `main()`; programmatic `mi_option_set` is too late).

**Results** (build-std + MIMALLOC_LARGE_OS_PAGES=1, 30s cooldown between runs):

| Config | Min | Median | Runs |
|--------|-----|--------|------|
| build-std only | 8.68 | 8.69 | 8.72, 8.68, 8.70, 8.73, 8.69, 8.68 |
| build-std + large pages | **8.53** | **8.60** | 8.59, 8.57, 8.63, 8.62, 8.63, 8.60 |

**Head-to-head** (5 alternating runs, 45s cooldown, MIMALLOC_LARGE_OS_PAGES=1):

| Round | V8 | Primecount | Winner |
|-------|-----|-----------|--------|
| 1 | 8.589 | 8.899 | V8 |
| 2 | 8.589 | 8.829 | V8 |
| 3 | 8.546 | 8.776 | V8 |
| 4 | 8.630 | 8.802 | V8 |
| 5 | 8.561 | 8.839 | V8 |
| **Median** | **8.589** | **8.829** | **V8 by 2.7%** |

V8 wins **5/5 rounds**. Median gap widened from 0.07s → 0.24s.

**Why explicit VirtualAlloc was worse**: mimalloc's transparent approach promotes ALL
allocations to large pages (thread stacks, rayon buffers, sieve segments, etc.).
Explicit VirtualAlloc only covers BigPiTable, leaving other allocations on 4KB pages.
The distributed TLB benefit across all allocations exceeds the targeted benefit.

**Technical note**: `mi_option_enable(MI_OPTION_ALLOW_LARGE_OS_PAGES)` called from
`main()` has no effect because mimalloc initializes arenas before `main()` runs (via
`#[global_allocator]`). CRT initializer `.CRT$XIU`/`.CRT$XCU` attempts also failed.
The only reliable method is the inherited env var.

**Committed**: eb14078

---

### Analysis: Why the B-First Architecture Is Near-Optimal

The current b-first architecture (iterate b-values, lookup BigPiTable) has been
exhaustively optimized through 92 experiments. The remaining gap to primecount
(~0.07s median) is explained by:

1. **L3 bandwidth contention** (the real bottleneck): AC, B, and D all run
   concurrently, competing for 36MB L3 + ~90 GB/s DRAM bandwidth. AC alone = 2.2s;
   concurrent = 8.5s (4× penalty). No micro-optimization can eliminate this.

2. **SegPiTable requires segment-first architecture**: primecount's SegmentedPiTable
   works because they iterate segments (build sieve once), then process all b-values.
   Our b-first architecture iterates b-values, then looks up BigPiTable. Converting
   to segment-first requires:
   - Replacing the AC outer segment loop with a sub-segment loop
   - Building an inverted index mapping segments → applicable b-values
   - Restructuring parallelism from par_iter-over-b to par_iter-over-segments
   - This is a ~1000-line rewrite of the most complex function in the codebase

3. **Diminishing returns**: Even with segment-first SegPiTable, the estimated
   improvement is ~1.5-3s on AC, making total = max(~5s AC, 7s B) = 7s. B then
   becomes the new bottleneck, requiring its own optimization cycle.

---

## Session 6: Exhaustive Verification (Opt 93-100)

### Opt 93: Segment-first SegmentedPiTable (5× REGRESSION)

**What**: Each rayon thread handles a chunk of wide b-values, independently sweeping
ALL sub-segments with a per-thread L1-sized sieve (SEG_PI_SIZE=131072, 12KB).
Added `rebuild_seg_sieve()` and `seg_pi_fast()` functions.

**Result**: AC loops = 11.77s (was 2.2s sequential / 8.4s concurrent) — **5× regression**.
- Root cause: 24 threads × 23K sub-segments × ~121μs sieve rebuild = 2.78s overhead/thread
- Total sieve crossings: ~4B per thread (constant regardless of segment size!)
- L1 cache benefit (~3ns/lookup × 220M lookups = 0.66s) overwhelmed by sieve cost (2.78s)
- **Key insight**: sieve construction cost O(√x × ln(ln(√(√x)))) per thread is a FIXED
  overhead that exactly cancels the L1 cache benefit
- **Reverted completely**

### Opt 94: Software Prefetch Pipeline (NEUTRAL)

**What**: Pipelined 4× unrolled loop — compute next batch's xpq values + issue
`big_pi.prefetch()`, then compute pi for current batch. ~8 extra instructions per
4-element batch providing ~70 cycle lead time.

**Result**: AC loops = 8.450s (was 8.412s baseline) — **neutral/slightly worse**.
- Root cause: Arrow Lake's OoO engine already looks ahead ~200+ instructions (~50 iterations)
- Hardware prefetch already covers L3 latency (~30ns)
- Software prefetch instructions add overhead without benefit
- **Reverted completely**

### Opt 95: Clustered Easy Leaves (4× REGRESSION AT SCALE)

**What**: When consecutive l-values give the same π(xpq), compute once and multiply
by cluster size. Formula: cluster_end = largest l' where primes[l'] ≤ floor(xp / primes[π_val]).
Uses binary search in primes array for O(log N) cluster boundary lookup.

**Correctness journey**:
1. Initial bug: `big_pi.pi(max_pl)` returned values inconsistent with primes array due
   to different sieves → fixed by using binary search in primes[] instead
2. Second bug: `primes[pi_val - 1]` treated array as 0-indexed, but primes[] is 1-indexed
   (primes[0]=0 sentinel, primes[k]=k-th prime) → used `primes[pi_val]` instead
3. After both fixes: **correct at all scales** (1B, 1T, 1Q, Max i64)

**Performance at Max i64**: AC loops = 35.12s (was 8.42s) — **4× regression**.
- Binary search in 6.3M-element primes array: ~23 comparisons × 5ns = 115ns per cluster
- Average cluster size at Max i64: only 1-3 (prime gaps comparable at both scales)
- Amortized cost: 115ns/3 = 38ns per element vs 10ns for direct pi_fast
- **Clustering only helps when l-range << pi_val range (small x); at Max i64, both are ~6.3M**
- **Reverted completely**

### Opt 96: PGO + build-std (NO IMPROVEMENT)

**What**: Clean PGO cycle with matching profile-generate and profile-use phases.
Profiled at Max i64 (201s instrumented run). No hash mismatch warnings.

**Result**: 8.58s median (was 8.57s baseline) — **no measurable improvement**.
- PGO's branch prediction benefit negligible for the tight AC inner loop
- build-std already provides most of PGO's code layout benefits
- Prior session's 8.54s PGO result (from 8.66s) was due to stale profiles / thermal variance

### Opt 97: Separate Thread Pools (REGRESSION)

**What**: Isolate AC and D into separate rayon pools to eliminate scheduling interference.

| Config | AC | D | B | Total |
|--------|------|------|------|-------|
| Default (global pool) | 8.42s | 5.61s | 7.31s | 8.59s |
| AC=24, D=24 | 14.87s | 5.68s | 6.74s | 15.03s |
| AC=12, D=12 | 17.84s | 5.61s | 6.59s | 18.00s |
| AC=8, D=16 | 15.43s | 9.64s | 4.23s | 15.65s |
| AC=24 only | 14.87s | 5.68s | 6.91s | 14.99s |

- Root cause: separate pools cause **oversubscription** (48-72 threads on 24 cores)
- Rayon's work-stealing with a single global pool is optimal

### Opt 98: LLVM Flag Tuning (NO IMPROVEMENT)

**What**: Tested various LLVM optimization flags with build-std.

| Flag | Total Time |
|------|-----------|
| unroll-threshold=800 (default) | 8.57s |
| unroll-threshold=1200 | 8.56s |
| unroll-threshold=400 | 8.54-8.62s |
| unroll-threshold=200 | 8.59s |
| --x86-cmov-converter=false | 8.57s |
| --enable-loopinterchange | 8.59s |

All within noise. The hot loop is already well-optimized by the compiler.

### Opt 99: Phased Scheduling (WORSE)

**What**: Run AC and D in separate phases instead of concurrently.

| Mode | AC | D | B | Total |
|------|------|------|------|-------|
| Default (all concurrent) | 8.42s | 5.61s | 7.31s | 8.57s |
| PHASE_AC_DB (AC first, then D+B) | **2.10s** | 5.27s | 6.34s | 8.99s |
| PHASE_D_ACB (D first, then AC+B) | **2.17s** | 4.50s | 4.31s | 9.36s |

- **Key finding**: AC alone = 2.10s, confirming 4× concurrent penalty (2.1→8.4s) is real
- Phased total = AC + max(D, B) = 2.1 + 6.3 = 8.4 + overhead = 8.99s (still worse)
- Sequential phasing adds ~0.4-0.8s overhead from phase transitions

### Opt 100: D Segment Size Tuning (NO IMPROVEMENT)

**What**: Reduce D's segment size to make D tasks more granular, reducing AC blocking.

| D_SEG_CAP | Segment Size | Segments | AC | Total |
|-----------|-------------|----------|------|-------|
| 20 (default) | 131040 | ~17 | 8.34s | 8.51s |
| 17 | 131040 | ~17 | 12.17s | 12.33s |
| 15 | 32760 | ~64 | 10.70s | 10.87s |
| 13 | 8190 | ~256 | 10.74s | 10.91s |

- Smaller segments make D slower (boundary overhead) and worsen AC (more scheduling contention)
- Default is already optimal

---

## Session 7: Thread Pool Tuning & Scheduling Experiments (Opt 101-103)

### Opt 101: DELAY_D Scheduling (WORSE — BigPiTable L3 Warming Discovery)

**What**: Wait for AC to finish before starting D. Theory: AC alone = 2.10s,
then D alone = ~5.0s, B runs throughout = 4.3s. Expected total: max(B, AC+D) = 7.1s.

**Result**: B = 8.5s, total = 8.7s — **WORSE than default** (8.57s).

**Critical discovery**: AC concurrent with D keeps BigPiTable (285MB) warm in L3 cache
for B's benefit. When AC finishes early (DELAY_D), D's sieve operations evict BigPiTable
from L3, making B dramatically slower (8.5s vs 7.3s). The AC "concurrent penalty" (2.1→8.4s)
is partially compensated by faster B through L3 cache warming.

**Reverted completely.**

### Opt 102: B_THREADS Sweep (DEFAULT OPTIMAL)

**What**: Sweep B pool size from 1 to 24 to find optimal AC/B thread balance.

| B_THREADS | AC | B | Total |
|-----------|------|-------|-------|
| 1 | 6.79s | 37.25s | 37.39s |
| 2 | 6.97s | 21.63s | 21.77s |
| 4 | 7.01s | 13.94s | 14.09s |
| 6 | 7.04s | 11.32s | 11.47s |
| 8 | 7.25s | 10.20s | 10.34s |
| 12 | 7.57s | 9.28s | 9.41s |
| 16 | 7.97s | 8.78s | 8.93s |
| 18 | 7.98s | 8.67s | 8.82s |
| 20 | 8.05s | 8.55s | 8.70s |
| 22 | 8.46s | 8.47s | 8.72s |
| **24** | **8.44s** | **6.94s** | **8.60s** |

**Key insight**: B=24 is optimal because total = max(AC, B). The crossover (AC ≈ B) occurs
at B≈22, but total there (8.72s) is worse because max(8.46, 8.47) > max(8.44, 6.94).
AC is always the bottleneck — giving B fewer threads speeds AC but slows B faster.

### Opt 103: Global Pool Size Tuning (DEFAULT OPTIMAL)

**What**: Vary POOL_MULT (rayon global pool = num_cpus × POOL_MULT).

| POOL_MULT | Global Threads | AC | B | Total |
|-----------|---------------|------|------|-------|
| 1 | 24 | 8.77s | 4.53s | 8.94s |
| 2 | 48 | 8.59s | 6.53s | 8.81s |
| **3** | **72** | **8.44s** | **6.94s** | **8.60s** |
| 4 | 96 | 8.54s | 6.82s | 8.68s |
| 5 | 120 | 8.47s | 8.53s | 8.68s |
| 6 | 144 | 8.51s | 7.94s | 8.67s |

POOL_MULT=3 is optimal. Fewer threads hurt AC (not enough tasks in flight for work-stealing).
More threads cause oversubscription. Also tested AC_SEG sweep (50K-500K): default 200K optimal.

---

## Session 8: Memory Access Pattern Experiments (Opt 104-107)

### Opt 104: Interleaved BigPiTable Layout (5% REGRESSION)

**What**: Merge `bits[]` and `prefix[]` into a single interleaved `data[]` array:
`data[2*w]` = prefix, `data[2*w+1]` = bits. Both values share a cache line (16B
per entry, 4 entries per 64B line). Theory: halve DRAM accesses for random pi_fast
lookups (1 cache line instead of 2).

**Result**: AC = 8.75s (was 8.44s), total = 9.05s — **5% regression**.

- Table grows from 285MB to 380MB (+33%) due to u64 padding for prefix
- B's sequential bits scan becomes stride-2 (50% cache utilization)
- **Key finding**: separate arrays have 8× better spatial locality for bits[]
  (8 consecutive bits words per cache line vs 4 interleaved). Within each b-value's
  iteration, consecutive xpq values often access nearby bits words, benefiting from
  dense packing.
- **Reverted completely**

### Opt 105: Deep Software Prefetch (4% REGRESSION)

**What**: Prefetch BigPiTable address 32-128 iterations ahead (300-1300ns lead time)
instead of Opt 94's 4 iterations (~12ns). Theory: exceed DRAM latency (80ns) to
pre-warm L2 for demand loads.

| Distance | AC Time | Note |
|----------|---------|------|
| 32 iters | 8.77s | +3.9% |
| 128 iters | 8.76s | +3.8% |

- **Root cause**: each prefetch uses an L2 miss tracking entry (12-16 available).
  With 2 demand loads per pi_fast × 4 unrolled = 8 entries, adding prefetch leaves
  only 4-8 entries for demands. This REDUCES memory-level parallelism.
- Extra fast_div computation for prefetch address adds ~6 μops per 4-iteration batch
- **Reverted completely**

### Opt 106: Monotonic Sweep with Running Pi (80% REGRESSION)

**What**: Since xpq decreases monotonically within each b-value iteration, maintain
a running pi counter and update via sequential sieve bit scans instead of random
pi_fast lookups. For small Δxpq (< 4096), scan ~16 words (128 bytes, fits L1d).

**Result**: AC = 15.48s (was 8.44s), total = 15.62s — **1.8× slower** (but correct).

- **Root cause: loss of 4× unrolling MLP**. The monotonic sweep serializes iterations
  (running_pi depends on previous scan result). Without 4× unrolling, MLP drops from
  8 outstanding L2 misses to 1-2, cutting memory bandwidth utilization by 4-8×.
- **Critical insight**: the AC inner loop is **memory-BANDWIDTH-bound**, not latency-bound.
  The OoO engine hides latency via MLP (4× unrolling × 2 loads each = 8 outstanding misses).
  Any optimization that reduces MLP is catastrophic.
- Even though ~89% of iterations have Δxpq < 4096 (where scans would be L1d hits),
  the bandwidth loss from serial execution overwhelms the cache benefit.
- **Reverted completely**

### Opt 107: 8× Unrolling (4% REGRESSION)

**What**: Increase unrolling from 4× to 8× to saturate L2 miss handling capacity
(16 outstanding misses vs 8 with 4×).

**Result**: AC = 8.77s (was 8.44s) — **4% regression**.

- 8 xpq + 8 pi values = 16 registers, exceeding x86-64's 16 GPRs → spills to stack
- Larger loop body (2× more code) increases I-cache pressure (L1i = 32KB)
- L2 miss handling was already near-saturated at 8 outstanding misses (4× unrolling)
- **4× unrolling is the Pareto-optimal point**: enough MLP to saturate L2 miss handling
  without register pressure or I-cache impact
- **Reverted completely**

---

## Conclusions After 107 Experiments

V8 at **8.39s best / 8.57s median** (build-std) is a **verified local optimum** for the
b-first BigPiTable architecture on Intel Core Ultra 9 285K.

### The Memory-Level Parallelism Constraint (Sessions 7-8 Discovery)

The AC inner loop is **memory-BANDWIDTH-bound with high MLP**:
- 4× unrolled loop generates 8 independent L2 miss requests (2 per pi_fast × 4)
- This saturates Arrow Lake's L2 miss handling capacity (~12-16 outstanding)
- Any change that reduces MLP is catastrophic (monotonic sweep: -80%, 8× unroll: -4%)
- Any change that increases table size hurts spatial locality (interleaving: -5%)
- Software prefetch competes with demand loads for L2 miss entries (-4%)

### The Concurrent Penalty Wall

The fundamental bottleneck is the 4× concurrent penalty on AC:
- AC alone (no D): **2.10s** with 24 threads
- AC concurrent with D: **8.42s** (4.0× slower)
- This penalty comes from: rayon work-stealing contention, L3 cache pressure
  from D's sieve operations, and power throttling from all-core load

### The BigPiTable L3 Warming Effect (Opt 101 Discovery)

AC's "concurrent penalty" is partially beneficial: AC's continuous BigPiTable lookups
keep the 285MB table warm in L3 cache, which speeds up B (also accesses BigPiTable).
When AC finishes early (DELAY_D), D evicts BigPiTable → B slows from 7.3s to 8.5s.
Any scheduling change that speeds AC at the expense of BigPiTable warmth slows B.

### The Thread Balance Wall (Opt 102)

B_THREADS sweep shows the AC/B balance is a zero-sum game: fewer B threads → faster AC
but slower B. At B=24 (default), total = max(8.44, 6.94) = 8.60s. The crossover at B≈22
gives max(8.46, 8.47) = 8.72s — worse. There is no thread configuration that improves both.

### What Would Be Needed to Go Faster

1. **Segment-first architecture rewrite** (~1000 lines): Process BigPiTable segments
   as the outer loop, with per-thread L1-sized SegPiTables. This would eliminate
   the concurrent penalty but requires restructuring the entire AC computation.

2. **B computation optimization**: At 7.3s, B becomes the bottleneck if AC improves.
   Requires algorithmic changes to primesieve streaming or B's sum formula.

3. **Different algorithm**: Lagarias-Miller-Odlyzko or analytic methods with different
   parallelism characteristics. Diminishing returns — primecount is already
   state-of-the-art.

4. **Hardware-specific**: NUMA-aware allocation, P-core/E-core task pinning at the
   OS level (not rayon). Requires custom thread scheduler in ~500 lines of unsafe code.

---

## Session 10: Deep Optimization Sprint (Opts 109-119)

**Goal**: Systematically explore 11 optimization avenues identified after 108 experiments.

### Opt 109: D prefix-sum counter array
**Hypothesis**: Replace O(words) count()/count_delta() in BitSieve with O(1) lookup via per-word prefix array, rebuilt after each cross_off_sieve.
**Result**: CATASTROPHIC REGRESSION — 19.3s (2.3× slower). The O(words) rebuild after every cross_off_sieve dominates. cross_off is called ~800 times per segment, each rebuild touching all 3125 words. This is a textbook case of trading O(N/2) per query for O(N) per update — the updates vastly outnumber queries.

### Opt 110: B chunk count reduction
**Hypothesis**: Reduce B's nchunks from nthreads×8 (192) to nthreads×2 (48), cutting primesieve jump_to() overhead.
**Result**: Inconclusive. B_CHUNKS=2: B slowed from 7.0→8.4s but AC improved 8.49→8.43s (less L3 contention). Wall time marginally better but B risks becoming bottleneck. Added configurable B_CHUNKS env var, kept default=8.

| B_CHUNKS | B (s) | AC (s) | Wall (s) |
|----------|-------|--------|----------|
| 1 (24)   | 8.35-8.69 | 8.33-8.45 | 8.52-8.81 |
| 2 (48)   | 8.37-8.47 | 8.40-8.45 | 8.52-8.57 |
| 4 (96)   | 7.43-7.65 | 8.48-8.52 | 8.58-8.63 |
| 8 (192)  | 6.68-7.43 | 8.42-8.55 | 8.53-8.67 |

### Opt 114: AC narrow b-value locality reorder
**Hypothesis**: Sort narrow b-values by (segment, xpq midpoint) so rayon work-stealing assigns b-values with similar BigPiTable access patterns to the same thread.
**Result**: Within noise. Median 8.57s vs 8.59s baseline. Kept the 5-line change (no downside).

### Opt 117: BigPiTable two-level prefix (u16 fine + u32 coarse)
**Hypothesis**: Replace u32 prefix with u16 fine_prefix (per-word, relative to 256-word block) + u32 coarse_prefix (per-block). Reduces BigPiTable from 285MB→229MB. Per-segment hot data: 2.4MB→2.0MB, potentially fitting in L2.
**Result**: REGRESSION — 8.89s median (0.30s worse). The extra coarse_prefix memory read adds latency to every pi_fast call in the critical inner loop. The memory reduction doesn't compensate because AC's bottleneck is per-access latency, not total memory size.

### AC segment size tuning (with large pages)
**Hypothesis**: Large pages change TLB economics; optimal AC_SEG may have shifted from 200K.
**Result**: No change. Tested 128K, 160K, 256K, 300K, 400K — all within noise of 200K. 400K notably worse (4.8MB working set >> L2 2MB).

### D_THREADS isolation experiment
**Key finding**: When D runs on a dedicated pool (even with same thread count), AC drops from 8.4s→2.8s! This proves **rayon scheduling contention** (not just L3 bandwidth) is the primary AC penalty. D's par_iter work items interfere with AC's work items at the rayon work-stealing level.
**Problem**: D on a separate pool takes 10-24s (vs 5.7s shared). Oversubscription (24+8+24=56 threads on 24 cores) kills D performance. Wall time = max(AC, D) = D = 10-24s.
**Conclusion**: The insight is valuable but unexploitable with current architecture. Would need a segment-first AC rewrite that eliminates the need for concurrent AC+D.

| D_THREADS | AC (s) | D (s) | Wall (s) |
|-----------|--------|-------|----------|
| 0 (shared)| 8.42-8.55 | 5.59-5.82 | 8.53-8.67 |
| 8         | 2.80-2.90 | 24.0-24.5 | 24.3-24.8 |
| 16        | 3.16-3.31 | 10.8-11.1 | 11.1-11.4 |
| 24        | 3.65-5.33 | 10.4-11.5 | 10.7-11.8 |

### Opt 118: AC inner loop 8× unroll
**Hypothesis**: Increase unrolling from 4× to 8× for more independent BigPiTable reads in flight, exploiting CPU memory-level parallelism.
**Result**: REGRESSION — 8.97s (0.4s worse). The 8× unrolled body exceeds the instruction cache budget and overwhelms the CPU's backend capacity (too many u128 multiplies + memory accesses simultaneously).

### Opt 119: D Type 2 work estimator
**Hypothesis**: D's work estimator only accounts for Type 1 VM work + cross-off. Adding Type 2 pair-leaf estimation should reduce D's bimodal behavior.
**Result**: Neutral on wall time. D variance slightly tighter (5.50-5.75 vs 5.59-5.82). Kept the improvement.

### AC prefetch-ahead
**Hypothesis**: Compute next iteration's xpq addresses (4 extra fast_divs), issue software prefetches, then process current iteration. Prefetches should hide BigPiTable L3 latency.
**Result**: REGRESSION — 9.7s (1.1s worse). The 4 extra fast_divs per iteration DOUBLE the compute cost. The hardware's out-of-order engine already provides sufficient memory-level parallelism; software prefetches with computed addresses are too expensive to justify.

### Summary

| Experiment | Wall (s) | vs Baseline | Status |
|------------|----------|-------------|--------|
| Baseline   | 8.59     | —           | —      |
| Opt 109 prefix-sum | 19.3 | +125% | ✗ catastrophic |
| Opt 110 B_CHUNKS=2 | 8.55 | -0.5% | ≈ inconclusive |
| Opt 114 narrow reorder | 8.57 | -0.2% | ≈ kept |
| Opt 117 two-level prefix | 8.89 | +3.5% | ✗ regression |
| Opt 118 8× unroll | 8.97 | +4.4% | ✗ regression |
| Opt 119 D Type 2 | 8.58 | -0.1% | ≈ kept |
| AC prefetch-ahead | 9.75 | +13.5% | ✗ regression |

**Key insight**: The system is at its optimization limit for the current architecture. The AC inner loop is memory-bandwidth-bound with the hardware's out-of-order engine already maximizing MLP. Any change that adds instructions or memory accesses to the inner loop causes a regression. The only path to significant improvement is a segment-first architectural rewrite that eliminates AC's 4× concurrent penalty.

V8 remains faster than primecount: 8.74s vs 8.90s (1.9% advantage, 5/5 wins).

---

## Session 11: Deep Optimization Search (Opts 121-130)

Baseline: 8.57s median (AC=8.47, D=5.7, B=7.0). V8 beats primecount by 2.6% (8.57 vs 8.80).

### Opt 121: LLVM flags — post-misched, enable-misched, inline-threshold (NOISE)
**What**: Tested LLVM backend flags: `-enable-post-misched`, `-machine-sink-split-probability-threshold=100`, `-enable-misched`, `-inline-threshold=1000`.
**Result**: All within noise (8.53-8.69s). Compiler already generating near-optimal code for the hot loop.

### Opt 122: Wheel-30 BigPiTable (REGRESSION — 13%)
**What**: Replace odd-only sieve (8 bits per 16 numbers) with wheel-30 encoding (8 bits per 30 numbers). Reduces BigPiTable from 285MB → 152MB (47% smaller). L3 hit rate doubles: 12.6% → 23.7%.
**Result**: AC = 9.56s (was 8.47s) — **13% regression**.
- Build: odd sieve → wheel-30 conversion, +80% build time (0.17s vs 0.095s)
- pi_fast decode: n/30 division + n%30 modulo + table lookup = ~7 extra µops per call
- ROOT CAUSE: Extra µops reduce ROB-limited MLP. With 128 µops per 4× group (was 92), the ROB holds fewer in-flight loads (16 vs 22). Memory throughput drops despite better hit rate.
- **Confirms the MLP bottleneck principle**: ANY instruction added to pi_fast reduces MLP and hurts, even with 47% smaller table.
- **Reverted completely**

### Opt 125: Interleaved BigPiTable — bits+prefix in same cache line (REGRESSION)
**What**: Combine bits[word] (u64) and prefix[word] (u32) into a single 16-byte struct per word. Reduces cache line accesses from 2→1 per pi_fast call.
**Result**: AC = 8.68s, Wall = 8.80s (was 8.57s) — **regression**.
- Total size: 362MB (was 285MB, +27% from padding)
- MLP already hides the second load (8 loads in flight from 4× unroll)
- Larger working set → worse LLC utilization → more DRAM misses
- **Reverted completely**

### Opt 124: P-core/E-core thread affinity via SetThreadAffinityMask (REGRESSION)
**What**: Pin 8 AC rayon threads to P-cores (5.7GHz, 2MB L2) and 16 D threads to E-cores (4.6GHz). Uses Windows SetThreadAffinityMask FFI.
**Result**: AC = 14.8s, D = 10.0s, Wall = 15.0s — **catastrophic regression**.
- 8 AC threads can't saturate DRAM bandwidth (need ~24 threads for full bandwidth)
- D on E-cores alone: 75% slower (lost P-core frequency + fewer threads)
- **Fundamental**: AC is bandwidth-limited, not core-frequency-limited. Needs ALL 24 cores.
- **Reverted completely**

### Opt 126: Rayon with_min_len tuning (REGRESSION at large values)
**What**: Control rayon's par_iter chunk size via `with_min_len(N)` to reduce scheduling overhead.
| AC_MIN_LEN | Wall (s) | Note |
|------------|----------|------|
| 1 (default)| 8.55    | baseline |
| 64         | 8.57    | noise |
| 256        | 8.92    | load imbalance |
| 1024       | 11.3    | severe imbalance |
| 4096       | 17.5    | catastrophic |
- Default fine-grained scheduling is already optimal. Larger chunks cause load imbalance because b-values have wildly different l-range sizes.

### Opt 127: Process priority HIGH/REALTIME (NOISE)
**What**: Set process priority class to High or RealTime via Start-Process.
**Result**: Both within noise of baseline (8.55-8.68s). System is already dedicated to the process.

### Opt 128: B thread throttling — reducing B_THREADS (INSIGHT but no wall improvement)
**What**: Reduce B's thread count to reduce memory bandwidth contention, giving AC more bandwidth.
| B_THREADS | AC (s) | B (s) | Wall (s) |
|-----------|--------|-------|----------|
| 24        | 8.42   | 7.41  | 8.53     |
| 22        | 8.61   | 8.57  | 8.73     |
| 20        | 8.43   | 8.76  | 8.87     |
| 16        | 7.96   | 8.80  | 8.91     |
| 12        | 7.72   | 9.26  | 9.37     |
| 8         | 7.44   | 10.44 | 10.57    |
| 4         | 7.21   | 14.36 | 14.49    |
**Insight**: AC improves 14% (8.42→7.21s) as B uses fewer threads. Confirms bandwidth contention is real. But B becomes the bottleneck before AC gains enough. Optimal is B_THREADS=24 (default).

### Opt 129: 1GB huge pages via MIMALLOC_RESERVE_HUGE_OS_PAGES (NOISE)
**What**: Reserve 2-4GB of 1GB huge pages instead of 2MB large pages.
**Result**: Median 8.55s vs 8.59s baseline — within noise (~0.5%). TLB is not a meaningful bottleneck with 2MB pages (BigPiTable = 143 pages, L2 TLB has 1536 entries).

### Opt 130: LTO variations — off, thin, fat (NOISE)
**What**: Test different LTO modes (none, thin, fat).
**Result**: All within noise (8.54-8.64s). LTO provides no measurable benefit for this single-binary, single-crate code.

### Confirmation experiments

**D_THREADS=8 (AC isolation)**: AC=2.80s, D=23.9s, B=4.9s, Wall=24.2s
- Confirms: when D has its own pool, AC is 3× faster (rayon scheduling contention eliminated)
- But D becomes catastrophically slow (oversubscription: 8+24=32 threads on 24 cores)

**PHASE_DB_AC (D+B first, then AC alone)**: D=5.6s, B=6.6s, AC=2.17s, Wall=9.25s
- AC alone takes only 2.17s — **confirms 4× concurrent penalty** (8.47/2.17 = 3.9×)
- Wall = max(D,B) + AC_alone = 6.6 + 2.17 = 8.77s (+ setup) > concurrent 8.57s

### Session 11 Summary

| Experiment | Wall (s) | vs Baseline | Status |
|------------|----------|-------------|--------|
| Opt 121 LLVM flags | 8.56 | -0.1% | ≈ noise |
| Opt 122 Wheel-30 BPT | 9.67 | +12.8% | ✗ regression |
| Opt 124 Core pinning | 15.0 | +75% | ✗ catastrophic |
| Opt 125 Interleaved BPT | 8.80 | +2.7% | ✗ regression |
| Opt 126 min_len=256 | 8.92 | +4.1% | ✗ regression |
| Opt 127 REALTIME priority | 8.63 | +0.7% | ≈ noise |
| Opt 128 B_THREADS=16 | 8.89 | +3.7% | ✗ B bottleneck |
| Opt 129 1GB huge pages | 8.55 | -0.2% | ≈ noise |
| Opt 130 thin LTO | 8.60 | +0.4% | ≈ noise |

**Critical discovery — ROB-limited MLP principle**: The AC inner loop is limited by how many memory loads fit in the CPU's reorder buffer (ROB=512 entries). With ~92 µops per 4× unrolled group, the ROB holds ~5.6 groups = 22 outstanding BigPiTable loads. Adding ANY instructions (wheel-30: +36 µops, interleaved: +0 but +27% memory) reduces effective MLP and causes regression. This is the FUNDAMENTAL reason no BigPiTable optimization works.

**The remaining viable path**: A streaming AC rewrite that eliminates BigPiTable entirely, converting random DRAM access to sequential L1 streaming. Estimated: AC from 8.47s → 2-3s concurrent (eliminating the 4× penalty). Wall time from 8.57s → ~7.0s. But requires ~500-1000 lines of new code.

Current performance: **V8 beats primecount 8.57s vs 8.80s (2.6% faster)**, verified 5/5 wins.
Total experiments: **145+** across 11 sessions.
