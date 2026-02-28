# THOUGHT LOG V10

## 2026-02-27

- Started V10 from V9 as a separate file: `src/bin/prime_count_v10.rs`.
- Preserved V9 files untouched.

### Redesign attempted

- Added contention-aware D scheduling:
  - AC starts as before.
  - Main thread can wait up to `D_WAIT_MS` before launching D.
  - Wait exits early if AC finishes (`Arc<AtomicBool>`).

### Findings

- Single-run sweeps can falsely suggest wins (e.g., `D_WAIT_MS=200`).
- Alternating median tests showed delay is not robust and tends to regress.
- Best default is still `D_WAIT_MS=0`.

### Comparative results

- V10 shows tiny median improvements vs V8/V9 in some run sets.
- But V10 repeatedly shows heavier slow outliers, harming mean consistency.

## 2026-02-27 (continued)

### Additional redesign pass

- Added adaptive D chunking policy driven by estimated segment-work skew.
- Kept D-start delay mechanism but defaulted `D_WAIT_MS=0` after repeated regressions.

### What worked

- `D_ADAPT_CHUNKS=1` improved V10 median vs fixed chunking in alternating tests.
- Base `D_CHUNKS` tuned down to 16 under adaptive policy.

### Comparative outcome

- V10 now beats V9 median and mean in 11-pair alternating validation.
- V10 also beats V8 median and mean in 11-pair alternating validation.

### Current interpretation

- V10 is now a small but credible step beyond V9 on this hardware.
- Remaining headroom still appears limited without deeper algorithmic/dataflow redesign.

## 2026-02-27 (next continuation)

### What I added

- Runtime auto-tune controller (`AUTO_TUNE`) in V10 that chooses config by scale, with env overrides preserved.
- Conservative adaptive D chunking revision:
  - switched from max-skew to p95-skew logic
  - added timing diagnostics (`SHOW_TIMING`) for chunking decisions

### Findings

- Max-skew was extremely noisy (`skewMax ~159`) and caused overreaction.
- p95-skew gave saner chunk choices (`eff=16` on diagnostic run), but overall median outcomes remained inconsistent in alternating tests.
- In latest 7-pair V9 vs V10 sample, V10 lost by ~0.26% median.

### Current interpretation

- V10 now contains useful experimental scheduling controls, but no stable production win from this continuation pass.
- V9 remains the reliable default for now.

## 2026-02-27 (continued again)

### Tried and reverted

1. **AC microkernel split** (separate C2/A kernels, branch-hoisted)
- Severe regression (~+7% median in 7-pair check).
- Reverted.

2. **B x/p compression** (run-length encoded quotient stream)
- Significant regression (~+2.3% median in 7-pair check).
- Reverted.

### Adaptive scheduler refinement

- Switched D adaptive chunking trigger from max-skew to p95-skew.
- Behavior is more reasonable diagnostically, but end-to-end wins remain unstable.

### Current state

- No additional robust gain from this pass.
- V10 remains experimental; V9 stays the stable baseline.

## 2026-02-27 (continued yet again)

### Counter-driven-adjacent refinement

- Reworked adaptive D policy to use sampled p95 skew instead of max skew.
- Added diagnostics to observe adaptive decisions in real runs.
- Behavior became less pathological, but overall speed impact remained unstable.

### Brute-force scheduler retuning

- Ran grid over `D_ADAPT_CHUNKS` + `D_CHUNKS`.
- Best fixed candidate emerged as `D_ADAPT_CHUNKS=0`, `D_CHUNKS=24`.
- In 7-pair checks this candidate beat both V9 and V8 medians, but noise remained significant.

### B thread follow-up

- Retested `B_THREADS`; 28 had tiny median shifts but worse tails.
- Not worth promoting as default.

### Working default stance

- Keep V10 as fixed-scheduler tuned mode (adapt off by default, D_CHUNKS=24, AUTO_TUNE off).
- Continue only with major new mechanisms if further gains are required.

## 2026-02-28 (continuation)

- Re-validated compile parity requirements from V8 docs:
  - nightly + `-Zbuild-std=std,panic_abort` + `x86_64-pc-windows-msvc`
  - `.cargo/config.toml` native/Arrow Lake rustflags active.

### Paths tested this pass

1. Lightweight D scheduler model (`D_WORK_MODEL=1`):
- Intended to reduce D chunk-planning overhead.
- Regressed hard (~8.5% median in alternating runs).
- Fully removed from code.

2. AC dedicated pool retest (`AC_THREADS`):
- Still catastrophic (15-20s range).
- Confirms this path is exhausted.

3. B thread retune revisit (`B_THREADS=24` vs `28`):
- Tiny median movement but worse tails at 28.
- Not promoted.

4. D chunk revisit (`D_CHUNKS=20` vs `24`):
- V10-only pairing favored 20 in one sample,
  but V9 cross-check contradicted it.
- Kept default at 24.

### Current assessment

- No new robust default win found in this continuation.
- Best practical V10 defaults remain unchanged (`D_CHUNKS=24`, adapt off, auto-tune off).
- Remaining plausible gains likely require deeper architecture changes, not further local knob sweeps.
