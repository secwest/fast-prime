# V6 Optimization Log — Enhanced DR with Segmented Pi Table

## Architecture

V6 builds on V5 (Deleglise-Rivat) with a key innovation from the Gourdon algorithm:
**segmented π-table processing** for S2_easy.

Same formula: π(x) = S1 + S2_easy + S2_hard + π(y) - 1 - P2

### The Problem V6 Solves

In V5, S2_easy performs random lookups into π[0..y], where y can be up to 19M at 10^18.
The π-table at y=19M is 76MB — far exceeding the 36MB L3 cache. This creates two issues:

1. **Random DRAM lookups** — Each π(x/(p·q)) access may miss L3, hitting DRAM at ~100ns
2. **Y-cap workaround** — V5 caps y at 9M so π-table fits in L3 (36MB), but this doubles z = x/y, increasing S2_hard work

### V6's Solution: Segmented Pi Table

Instead of random access across 76MB, V6 divides π[0..y] into 512K-entry segments (2MB each, fits in L2 cache). For each segment [low, high):

1. All π lookups within that segment hit L2 cache (~5ns vs ~100ns)
2. For each prime b, computes the valid l range where x/(p_b·p_l) ∈ [low, high)
3. Processes only the relevant (b, l) pairs — same total work, but cache-friendly

### Adaptive Dispatch

- **Pi table ≤ L3 (36MB, y ≤ 9M)**: Uses V5's direct approach (parallel over b, software prefetch)
- **Pi table > L3**: Uses segmented approach (parallel over segments)

This ensures optimal performance at all scales.

---

## Opt 0 — Baseline: Segmented Pi Table + Uncapped Y

**What**: Created V6 with segmented S2_easy and removed the y=9M cap from V5.

**Implementation**:
- `compute_s2_easy_segmented()`: Parallel over segments via rayon. Each segment is 512K entries (2MB).
  For each segment, iterates over valid b values with narrowed b-range (max_b_seg = π(√(x/seg_low))).
  Uses clustering optimization within each segment for batch evaluation.
- `compute_s2_easy_direct()`: Copy of V5's S2_easy with prefetch, used when pi table fits in L3.
- `compute_s2_easy()`: Dispatcher — chooses direct vs segmented based on `(y+1)*4 ≤ 36MB`.
- `count_primes()`: No y-cap. Alpha curve unchanged, y determined purely by α·∛x.

**Results (V6 Opt 0 vs V5 Opt 5)**:

| Range | V5 | V6 | Speedup | Notes |
|-------|-----|-----|---------|-------|
| 1 Billion | 0.0008s | 0.002s | 0.4× | V5 faster (direct path) |
| 10 Billion | 0.003s | 0.002s | 1.7× | Slight improvement |
| 100 Billion | 0.005s | 0.003s | 1.7× | Direct path used |
| 1 Trillion | 0.009s | 0.007s | 1.4× | Direct path used |
| 10 Trillion | 0.029s | 0.025s | 1.2× | Direct path used |
| 100 Trillion | 0.092s | 0.089s | 1.03× | Direct path used |
| 1 Quadrillion | 0.539s | 0.518s | 1.04× | Direct path used |
| 10 Quadrillion | 3.41s | 3.52s | 0.97× | Near breakeven |
| 100 Quadrillion | 21.00s | 20.72s | 1.01× | Near breakeven |
| **1 Quintillion** | **172.4s** | **113.4s** | **1.52×** | Segmented pi kicks in! |
| **Max i64** | — | **547.3s** | — | First V6-only measurement |

**Analysis**:
- For scales ≤ 100Q: Pi table fits in L3 → V6 uses direct path → performance ≈ V5
- At 1 Quintillion: y=19M uncapped (vs V5's 9M cap), pi table=76MB → segmented approach
  - S2_easy: segmented pi gives L2-fast lookups instead of DRAM random access
  - S2_hard: z halved (53B vs 111B) → ~50% fewer sieve segments
  - Combined: **34% faster** (113.4s vs 172.4s)
- Max i64 (9.2×10^18): 547.3s, first measurement for V5/V6 algorithm at this scale
  - V4 was 939.2s → V6 is **1.72× faster than V4**

**Key insight**: The segmented approach eliminates the fundamental tension between
S2_easy cache efficiency and S2_hard work volume. V5 had to cap y at 9M to keep the
pi table in L3, doubling S2_hard work. V6 processes the pi table efficiently at any
size, allowing y to grow naturally, which dramatically reduces S2_hard.
