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

## 2026-03-01 (continuation)

- Performed long-window validations (11 alternating pairs) to reduce false-positive tuning signals.
- Re-tested all remaining scheduler knobs not fully exhausted in recent passes.

### What held up

- Current fixed default mode (`D_CHUNKS=24`, `D_ADAPT_CHUNKS=0`) beat V9 by ~0.30% median and also improved mean in the longest run set this pass.

### What did not hold up

- Adaptive candidate (`D_ADAPT_CHUNKS=1`, `D_CHUNKS=28`) looked good in shorter samples but collapsed to near-tie median and worse mean in 11-pair validation.
- `D_AUTO_CHUNK_SELECT`, tiny D delays, alternative `D_SEG_CAP`, and non-default pool multiplier remained non-robust or worse.
- Two code edits attempted (AC narrow pre-bucketing and D monotonic VM-hints) both regressed and were reverted.

### External research check

- Scanned current references and emerging literature.
- Noted a 2024 O(sqrt(n)) prime-counting method (AMS MCOM page), but this implies a major algorithm branch, not an incremental V10 optimization.
- No direct low-risk internet-sourced speedup found for immediate integration.

### Current conclusion

- For V10 as currently structured, local optimization paths are now close to exhausted.
- Remaining paths forward are architecture-level rewrites, not parameter or microkernel tuning.

## 2026-03-01 (later continuation)

- Added and retained one concrete V10 code optimization:
  - fused D correction pass and prefix update into one loop.
- A/B against previous V10 favored this change on median in both 7-pair and 11-pair tests.
- Tried additional manual unrolling on top of fused loop; it regressed and was reverted.

Current retained code delta from previous checkpoint:
- D correction pass now performs one combined pass instead of two passes.

Current status:
- V10 still highly noise-sensitive versus V9 at this margin, but this is a defensible local win versus prior V10 implementation.
- Further improvements likely require larger algorithmic restructuring rather than additional loop-shape micro-tuning.

- Tested an additional micro-optimization on top of fused loop (skip `bb<=c`), but it was effectively noise and worsened mean; reverted.
- Retained only the fused correction/pass update change as net code improvement in this continuation.

## 2026-03-01 (next continuation)

- Investigated D segment floor because timing showed very high D segment count.
- Added `D_SEG_MIN_CAP` knob only; left default behavior unchanged.

### Findings

- Raising min segment floor can improve V10-vs-V10 medians in some long alternating runs.
- Cross-checks versus V9 remained mixed at median even when mean improved.
- Default `D_SEG_MIN_CAP=17` remains safer.

### Paths exhausted this pass

1. Uniform D chunking architecture (`D_UNIFORM_CHUNKING`) was catastrophic and removed.
2. Dedicated `D_THREADS` retest remained slower than default.
3. C1 pool configurability experiments produced no robust production gain and were reverted.

### Net

- One small retained code change: D min segment floor is now tunable by env var.
- No default knob changes from this pass.

## 2026-03-02 (continuation)

- Ran another broad pass focused on D setup/modeling and scheduler phase variants.
- No new path produced a robust median win suitable for changing defaults.

### What was tried

1. Gating skew-stat computation when adapt/auto are off:
- Regressed in balanced A/B; reverted.

2. Removing D per-chunk max_b pre-scan (lazy vector growth):
- Regressed; reverted.

3. Re-sweep of all built-in `PHASE_*` modes:
- All slower than default schedule.

4. Wider grid near current defaults (`D_SEG_MIN_CAP`, `D_ADAPT_CHUNKS`, `D_CHUNKS`):
- Found fast single runs, but long alternation did not confirm a median improvement.

### Current interpretation

- V10 is near a local optimum under current architecture; remaining deltas are highly noise-sensitive.
- Further gains likely require deeper algorithmic/dataflow changes, not more local scheduler micro-tuning.

## 2026-03-02 (breakthrough continuation)

- Identified and retained a new code-level optimization in AC:
  - segment-level serial fallback threshold (`AC_PAR_MIN`) to reduce rayon overhead on small per-segment work.
- Tuned default threshold to `256` after sweep + alternating checks.

### Evidence summary

- New V10 beats prior V10 in 11-pair alternating validation (median and mean).
- New V10 also shows a strong 11-pair win vs V9 in this run window (~0.8% median).

### Rejected in same pass

- D lazy vector growth (regression).
- D estimator sample reduction (no robust gain).
- D skew-stat gating (regression).
- Phase-mode and broader D-grid rechecks (no robust default change).

### Current state

- AC_PAR_MIN=256 is now the primary retained improvement from this continuation.
- Other defaults unchanged unless documented previously.

## 2026-03-02 (latest correction pass)

- Re-audited `AC_PAR_MIN` tuning with longer alternation windows.
- Found that nonzero defaults (192/256) are not robust enough despite earlier promising samples.
- Kept AC_PAR_MIN feature, but defaulted it back to `0`.

### Important net result

- Even with `AC_PAR_MIN=0`, post-refactor AC path still outperforms pre-refactor V10 in long A/B.
- Latest 11-pair run versus V9 showed a strong margin for current V10 (~1.0% median in that window).

### Current state

- AC_PAR_MIN remains available as an experimental knob.
- Production default uses full parallel AC segment processing (`AC_PAR_MIN=0`).

## 2026-03-03 (stability continuation)

- Re-ran long-window validations specifically to test if recent apparent wins were robust.

### Confirmed

- The AC_PAR_MIN feature is useful, but nonzero defaults (`192/256`) were not stable.
- Default `AC_PAR_MIN=0` is the safer production stance and still improves over pre-feature V10.

### Rejected

- D p95 selection via `select_nth_unstable` (regression).
- D skew sample-size retuning (no robust gain).
- Additional knob shifts (`D_CHUNKS`, `AC_SEG`) remained too noise-sensitive for default changes.

### Outcome

- No code delta retained in this pass.
- Defaults remain as previously documented.

## 2026-03-03 (later continuation)

- Explored additional deeper code paths in B and D; all code modifications regressed and were reverted.
- After long revalidation sweeps, one small but defensible default retune remained:
  - `B_CHUNKS` default moved from 4 to 2.

### Why this was kept

- It repeatedly produced a small median advantage in long alternating tests, with no mean penalty in the longest run.
- The change is low-risk and fully overrideable via env.

### Current state

- V10 continues to gain mostly through robust scheduling defaults and avoiding unstable micro-optimizations.
- Remaining possible wins likely require larger algorithmic changes rather than local loop rewrites.

## 2026-03-03 (latest continuation)

- Pushed through multiple failed deep-path attempts and focused on long-window validation quality.
- Landed one robust default improvement this pass:
  - `AC_SEG` default moved from `180000` to `160000`.

### Why kept

- 21-pair alternating validation favored `160000` on both median and mean.
- V9 cross-check with `160000` also remained favorable.

### Also synced

- Runtime top-tier auto-tune values now reflect current best defaults (`ac_seg=160000`, `b_chunks=2`).

### Rejected in this pass

- compute_b boundary sweep rewrite.
- D Type-2 monotonic l-cap rewrites.
- Additional D sample/tuning tweaks.
- AC fast-path branch micro-edit (no robust gain).
