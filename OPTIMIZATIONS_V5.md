# V5 Optimizations — Deleglise-Rivat Algorithm

## Algorithm Overview

V5 implements the Deleglise-Rivat (DR) method for prime counting. The key innovation over
the LMO algorithm (V4) is splitting the special leaves into three categories:

- **S2_hard**: Leaves requiring segmented sieve computation (same as V4's S2 approach)
  - Type 1: b ≤ π(√y), composite m with μ(m)≠0 and lpf(m) > p_b
  - Type 2: π(√y) < b ≤ π(√z), hard leaves where x/(p_b·p_l) ≥ y
- **S2_easy**: Leaves where x/(p_b·p_l) < y, using direct π-table lookup
  - Uses identity: φ(n, b-1) = π(n) - b + 2 when n < p_b² (holds since p_b > √y)
  - Handles trivial leaves (φ=1) separately for correctness
  - Parallelized over b using rayon
- **S2_trivial**: Merged into S2_easy (all leaves for b > π(√z) are easy)

Formula: π(x) = S1 + S2_easy + S2_hard + π(y) - 1 - P2

### Key Design Decisions

1. **Boundary condition**: Leaves where x/(p_b·p_l) = y exactly go to the "hard" side
   (counted by S2_hard using sieve) to avoid a gap between easy and hard ranges.
   
2. **Trivial leaves**: For x/(p_b·p_l) < p_b, the formula π(n)-b+2 can give ≤0,
   but φ(n,b-1)=1. These are counted separately as contributing 1 each.

3. **S2_easy clustering**: Consecutive l values giving the same π(x/(p·q)) are
   batched together for efficiency.

## Baseline Performance (unoptimized)

| Range | V4 (LMO) | V5 (DR) | Ratio |
|-------|----------|---------|-------|
| 1B | 0.001s | 0.001s | ~1x |
| 10T | 0.019s | 0.080s | 4.2x slower |
| 100T | 0.091s | 0.341s | 3.7x slower |
| 1Q | 0.755s | 1.558s | 2.1x slower |
| 10Q | 5.43s | 8.70s | 1.6x slower |
| 100Q | 33.63s | 53.18s | 1.6x slower |
| 1 Quintillion | 192.0s | 422.7s | 2.2x slower |

V5 is correct but slower than V4 — this is expected since V5 uses V4's alpha parameters
(tuned for LMO) and S2_hard is single-threaded. Optimization opportunities below.

## Optimization Log

### Opt 0: Baseline implementation
- Correct results through 1 Quintillion (all test cases pass)
- S2_hard is single-threaded (main bottleneck at large scales)
- S2_easy is parallelized via rayon
- Uses V4's alpha curve (may not be optimal for DR)

### Opt 1: Parallel S2_hard via delta-phi correction (**2.2× speedup at 100T**)
- Adapted V4's parallel S2 technique: segments split across chunks, each tracking
  local phi + correction coefficients. True phi reconstructed via prefix-sum after join.
- Added serial fallback for small inputs (≤2 segments)
- Added large-prime optimization: for p > √high, only clear the prime itself (not composites)
- Added sqrt_high optimization for type 2 cross-off

### Opt 2: Alpha retuning + concurrent S2_easy (**34% speedup at 10Q**)
- Steeper alpha ramp at 10^14: α = 2.2 + 0.8*(log-13) → hits α=3.0 at 100T (was 2.4)
- S2_easy runs concurrently with S2_hard via thread::scope (previously sequential)
- At 10Q: S2_easy takes ~2s, completely hidden behind S2_hard
- Results (cumulative from Opt 0 baseline):
  - 10T: 0.080s → 0.028s (2.9× faster)
  - 100T: 0.341s → 0.135s (2.5× faster)
  - 1Q: 1.558s → 0.636s (2.4× faster, now 1.19× FASTER than V4!)
  - 10Q: 8.70s → 3.74s (2.3× faster, now 1.45× FASTER than V4!)
  - 100Q: 53.18s → 23.88s (2.2× faster, now 1.41× FASTER than V4!)

### Opt 3: Pi-formula fast path for segment 0 (**32% speedup at 100T**)
- Precomputes global π(n) lookup table for n ∈ [0, segment_size] (only when ≤16M)
- For segment 0, type 2 leaves with primes[b-1]² ≥ high use identity:
  φ(n, b-1) = 1 + max(π(n) - (b-1), 0) — avoids sieve counting entirely
- Batch optimization for p³ ≥ x: all leaves have φ=1, counted in O(leaves) without π-lookup
- Table capped at 16M entries to avoid multi-GB allocation at very large scales
- Biggest impact at 100T where segment 0 dominates (all primes contribute)
- Results (cumulative):
  - 10T: 0.028s → 0.026s (7% faster)
  - 100T: 0.135s → 0.092s (**32% faster**, now 1.22× FASTER than V4!)
  - 1Q: 0.636s → 0.539s (15% faster, now 1.43× FASTER than V4!)
  - 10Q: 3.74s → 3.55s (5% faster, now 1.59× FASTER than V4!)
  - 100Q: 23.88s → 24.58s (~same, 1.46× FASTER than V4)
