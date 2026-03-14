# V10 Optimization Log

## Goal

Attempt a V10-class scheduling redesign beyond V9 micro-tuning, while keeping all V9 files intact.

## Architecture Attempt 1: Adaptive D-Start Delay

### Idea

Delay D start briefly after `build_vm` to reduce early AC-vs-D DRAM contention.

- New env: `D_WAIT_MS`
- Behavior:
  - Wait up to `D_WAIT_MS` before starting D
  - Exit wait early if AC completes
- Implementation uses `Arc<AtomicBool>` to track AC completion.

### Sweep (single-run signal, Max i64)

- `D_WAIT_MS=0`: 8.42148s
- `200`: 8.32930s (looked promising)
- `400`: 8.48655s
- `700`: 8.46734s
- `1000`: 8.47176s
- `1400`: 8.56199s

### Alternating validation

- 7-run alternating `0` vs `200`:
  - Median `0`: 8.38434s
  - Median `200`: 8.40623s
  - `200` is slower by ~0.261%

### Verdict

- Delay is not robust.
- Keep feature for experimentation but set default to `D_WAIT_MS=0`.

## Architecture Attempt 2: Adaptive D Chunking (SUCCESS)

### Idea

D chunk count should react to actual predicted segment-work skew instead of a fixed multiplier.

### Change

- Added `D_ADAPT_CHUNKS` (default enabled).
- After computing `work_per_seg`, estimate skew = `max_work / avg_work`.
- Select effective chunk multiplier:
  - skew >= 6.0 -> 40
  - skew >= 4.0 -> 32
  - skew <= 1.6 -> 16
  - else use base `D_CHUNKS`
- Tuned base `D_CHUNKS` default from 24 -> 16 for V10.

### Validation

- V10 internal alternating (7 runs, `D_ADAPT_CHUNKS=0` vs `1`):
  - Median no-adapt: `8.46097s`
  - Median adapt: `8.41826s`
  - Delta: `-0.505%`
- With adapt enabled, base `D_CHUNKS` tuning (7 runs `16` vs `24`):
  - Median `16`: `8.47673s`
  - Median `24`: `8.48695s`
  - Delta: `-0.120%`

## V10 vs V8/V9

### V8 vs V10 (11 alternating pairs, defaults)

- V8 median: `8.40321s`
- V10 median: `8.39198s`
- Delta: `-0.134%` (tiny median win)
- Means: V8 `8.41323s`, V10 `8.43619s` (V10 worse tail behavior)

### V9 vs V10 (11 alternating pairs, defaults)

- V9 median: `8.41141s`
- V10 median: `8.38856s`
- Delta: `-0.272%` (median better)
- Means: V9 `8.42137s`, V10 `8.42759s` (slightly worse mean)

### V9 vs V10 (11 alternating pairs, defaults after adaptive chunking)

- V9 median: `8.46141s`
- V10 median: `8.42999s`
- Delta: `-0.371%` (V10 faster)
- Means: V9 `8.45650s`, V10 `8.44796s` (V10 also slightly better)

### V8 vs V10 (11 alternating pairs, defaults after adaptive chunking)

- V8 median: `8.47166s`
- V10 median: `8.42604s`
- Delta: `-0.539%` (V10 faster)
- Means: V8 `8.47163s`, V10 `8.46006s`

## Conclusion

V10 now shows a modest but repeatable improvement over both V8 and V9 on this platform after adaptive D chunking. The D-start delay itself remains non-robust and stays disabled by default.

## Follow-up: Runtime Auto-Tuning Controller + Conservative Adaptation

### Added

- Runtime tuning controller (`AUTO_TUNE`) that can set:
  - `AC_SEG`, `B_CHUNKS`, `D_CHUNKS`, `D_ADAPT_CHUNKS`
  - still with env-var override priority
- Adaptive D chunking changed from max-skew driven to **p95-skew** driven to avoid overreacting to outlier segments.

### Observations

- `AUTO_TUNE=1` did not consistently beat fixed tuned defaults at Max i64.
- Conservative p95-based chunking produced cleaner decision signals (`eff` not forced high by extreme outliers), but end-to-end median gains were still inconsistent run-to-run.

### Latest checks

- V9 vs V10 (7 alternating, p95 adaptation):
  - V9 median: `8.41241s`
  - V10 median: `8.43442s`
  - Delta: `+0.262%` (V10 slower in this sample)

### Current practical stance

- Keep V10 adaptive features as experimental knobs.
- V9 remains the safer production baseline until V10 shows stable wins across larger controlled runs.

## Additional Attempts (This Pass)

### A. Hand-Tuned AC Microkernel Split (FAILED, reverted)

- Implemented separate branch-hoisted C2/A kernels with 4x unrolled loops.
- Result (7 alternating V9 vs V10): catastrophic regression.
  - V9 median `8.44453s`
  - V10 median `9.04992s`
  - `+7.169%` slower
- Reverted immediately.

### B. B x/p Compression (FAILED, reverted)

- Implemented run-length encoding of repeated `x/p` values with weighted merge.
- Result (7 alternating V9 vs V10): large regression.
  - V9 median `8.41872s`
  - V10 median `8.61123s`
  - `+2.287%` slower
- Reverted immediately.

### C. Adaptive D Chunking Robustness Tweaks (INCONCLUSIVE)

- Replaced max-skew trigger with p95-skew trigger for chunk multiplier selection.
- Reduced overreaction to outlier segments, but alternating benchmarks remained noisy and inconclusive.

## Net After Reverts

- V10 returns to previous adaptive scheduling baseline behavior (no proven stable win over V9 in this pass).

## Counter-Driven Scheduler Follow-up (Heuristic Form)

### What was attempted

- Reworked adaptive D chunking to use sampled `p95` segment-work skew (instead of `max` skew) to avoid overreacting to extreme outliers.
- Added `SHOW_TIMING` diagnostics for D chunk decisions (`base`, `adapt`, `eff`, `skew95`, `skewMax`).

### Result

- Diagnostics became more interpretable and less erratic.
- End-to-end gains remained inconsistent across alternating runs.

## Additional Parameter Exploration

### D scheduler grid (single-run signal)

- Swept `D_ADAPT_CHUNKS` x `D_CHUNKS` and validated top candidates.
- Best practical candidate remained:
  - `D_ADAPT_CHUNKS=0`
  - `D_CHUNKS=24`

### Candidate validation

- V9 vs V10 candidate (7 alternating):
  - V9 median `8.43172s`
  - V10 median `8.36718s`
  - Delta `-0.765%`
- V8 vs V10 candidate (7 alternating):
  - V8 median `8.43207s`
  - V10 median `8.38471s`
  - Delta `-0.562%`

### B thread retune attempt

- `B_THREADS=28` vs `24` in V10 showed only tiny median movement with worse tails.
- Against V9, net effect was near-noise (`~0.06%` median).
- Not adopted as default.

## Current V10 Defaults

- `AUTO_TUNE=0` (off by default)
- `D_WAIT_MS=0` (off)
- `D_ADAPT_CHUNKS=0` (off by default)
- `D_CHUNKS=24` default

## Current Assessment

- V10 remains sensitive to thermal/noise conditions, but best fixed scheduler settings can outperform V9 in multiple paired samples.
- No clear further low-risk incremental path remains without deeper instrumentation or a larger architecture shift.

## Continuation Pass: Additional Scheduler Avenues (Compile-Parity Retest)

Date: 2026-02-28

Build parity (same as V8 docs):
- `cargo +nightly build --release --bin prime_count_v9 --bin prime_count_v10 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
- Uses `.cargo/config.toml` native/tuning rustflags.

### Attempt: Lightweight D Work Estimator (`D_WORK_MODEL=1`) (FAILED, reverted)

What:
- Added optional low-overhead D scheduler work model intended to reduce chunk-planning cost.
- Compared against legacy estimator (`D_WORK_MODEL=0`) in 5 alternating runs.

Result:
- `model=0` median: `8.40493s`
- `model=1` median: `9.11775s`
- Delta: `+8.48%` (severe regression)

Action:
- Removed the `D_WORK_MODEL` path from V10 code.
- Kept legacy estimator only.

### Attempt: AC dedicated pool retest (`AC_THREADS`) (FAILED)

Single-run sweep showed catastrophic regressions with dedicated AC pool:
- `AC_THREADS=6`: `19.65s`
- `AC_THREADS=8`: `15.22s`
- `AC_THREADS=10-14`: ~`15.0s`

Conclusion:
- Confirms oversubscription/contention issue remains.
- Keep `AC_THREADS` default behavior (`0`, global pool path).

### Attempt: B thread retune revisit (`B_THREADS`) (NO ROBUST WIN)

5 alternating runs, `B_THREADS=24` vs `28`:
- `24` median: `8.41286s`
- `28` median: `8.41047s` (tiny)
- Means: `24` = `8.42077s`, `28` = `8.43319s`

Conclusion:
- Median difference is noise-level and tails worsen at 28.
- Keep default unchanged.

### Attempt: D chunk retune revisit (`D_ADAPT_CHUNKS=0`)

5 alternating runs, `D_CHUNKS=24` vs `20` (V10-only):
- `24` median: `8.47628s`
- `20` median: `8.41277s`
- Delta: `-0.749%` (looked promising)

Cross-check vs V9 (7 alternating):
- V9 median: `8.41438s`
- V10 (`D_CHUNKS=20`) median: `8.45409s`
- Delta: `+0.472%` (V10 slower)

Cross-check vs V9 with current `D_CHUNKS=24` (7 alternating):
- V9 median: `8.43953s`
- V10 (`D_CHUNKS=24`) median: `8.42184s`
- Delta: `-0.210%` (V10 slightly faster)

Conclusion:
- `D_CHUNKS=20` was not robust once validated against V9.
- Keep default `D_CHUNKS=24`.

## Current V10 Default Stance (unchanged)

- `AUTO_TUNE=0`
- `D_WAIT_MS=0`
- `D_ADAPT_CHUNKS=0`
- `D_CHUNKS=24`
- `D_AUTO_CHUNK_SELECT=0`

## Continuation Pass: Long-Window Validation + Emerging Research Scan

Date: 2026-03-01

Build parity:
- `cargo +nightly build --release --bin prime_count_v9 --bin prime_count_v10 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
- Existing `.cargo/config.toml` native/Arrow Lake rustflags.

### Scheduler/knob rechecks (this pass)

1. `D_AUTO_CHUNK_SELECT` (`0` vs `1`, 7 alternating, V10-only):
- `off` median: `8.37252s`
- `on` median: `8.38706s`
- Delta: `+0.174%` (worse)

2. `D_ADAPT_CHUNKS` (`0` vs `1`, 7 alternating, V10-only):
- `0` median: `8.47601s`
- `1` median: `8.43571s`
- Delta: `-0.475%` (looked good in this sample)

3. `D_SEG_CAP` sweep (`17..22`):
- Best single-run was default `20` (`8.30851s` in that sequence)
- 5 alternating `19` vs `20`:
  - `19` median: `8.54742s`
  - `20` median: `8.38844s`
  - Delta: `+1.895%` for `19` (much worse)

4. `POOL_MULT` sweep:
- `3` still best in this pass (`1` and `2` clearly slower; `4/5` slower)

5. `D_WAIT_MS` tiny-delay revisit (`0,25,50,75,100`):
- 5 alternating `0` vs `75`:
  - `0` median: `8.39726s`
  - `75` median: `8.42959s`
  - Delta: `+0.385%` (worse)

### Code-level attempts in this pass (both FAILED and reverted)

1. AC narrow-by-segment pre-bucketing (to remove per-segment binary searches)
- Direct A/B vs previous V10 (7 alternating):
  - old median: `8.40110s`
  - new median: `8.45910s`
  - Delta: `+0.690%` (worse)

2. D Type-1 monotonic VM bound hint reuse
- Direct A/B vs previous V10 (7 alternating):
  - old median: `8.39001s`
  - new median: `38.23653s`
  - Catastrophic regression; reverted immediately.

### Long-window cross-checks against V9 (11 alternating pairs)

A) V10 current default (`D_CHUNKS=24`, `D_ADAPT_CHUNKS=0`):
- V9 median: `8.40519s`
- V10 median: `8.37988s`
- Delta: `-0.301%` (V10 faster)
- Means: V9 `8.43595s`, V10 `8.37810s` (V10 better)

B) V10 adaptive candidate (`D_CHUNKS=28`, `D_ADAPT_CHUNKS=1`):
- V9 median: `8.40294s`
- V10 median: `8.40214s`
- Delta: `-0.010%` (essentially tie)
- Means: V9 `8.39851s`, V10 `8.41287s` (V10 worse)

### Emerging research / external scan

Checked for post-classic algorithmic advances likely to beat tuned Gourdon on this platform:
- Primecount project/docs baseline context: <https://github.com/kimwalisch/primecount>
- New theoretical direction found: an O(sqrt(n)) counting method in 2024 (AMS paper page):
  <https://www.ams.org/journals/mcom/2024-93-348/S0025-5718-2024-03986-2/>

Practical assessment for this codebase:
- The O(sqrt(n)) result is mathematically significant but not yet an obvious drop-in replacement for this highly tuned Max-i64 exact counter on CPU; engineering risk is high and likely requires a new branch/architecture rather than V10 incremental tuning.
- No low-risk, evidence-backed internet-sourced optimization emerged that can be integrated as a direct V10 speedup right now.

## Current V10 stance after this continuation

Unchanged defaults remain the best production choice in this pass:
- `AUTO_TUNE=0`
- `D_WAIT_MS=0`
- `D_ADAPT_CHUNKS=0`
- `D_CHUNKS=24`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`
- `D_SEG_CAP=20`

## Continuation Pass: D Correction-Pass Memory Traffic Reduction

Date: 2026-03-01 (later pass)

### Implemented: Fused correction + prefix update loop in D

What changed:
- In `compute_d`, correction pass previously did two scans over `0..limit` for each chunk boundary:
  1) `correction += prefix_phi[bb] * coeff[bb]`
  2) `prefix_phi[bb] += phi_total[bb]`
- Fused into a single pass performing both operations together.

Why:
- Reduces one full memory pass over large vectors per chunk merge.
- Expected to lower memory traffic/cache pressure in D post-processing.

Validation (old vs new V10, 11 alternating pairs, Max i64):
- old median: `8.40562s`
- new median: `8.38362s`
- Delta: `-0.262%` (new faster)
- Means: old `8.41170s`, new `8.41404s` (essentially flat/noisy)

Short-run check (7 alternating) also favored the fused loop:
- old median: `8.39782s`
- new median: `8.37109s`
- Delta: `-0.318%`

### Follow-up attempt: manual 4x unroll of fused loop (FAILED, reverted)

- Unrolled fused loop regressed in paired test:
  - old median: `8.40413s`
  - unrolled median: `8.42415s`
  - Delta: `+0.238%`
- Reverted to the simple fused loop.

### Cross-check versus V9 (7 alternating, new V10 code)

- V9 median: `8.37507s`
- V10 median: `8.39322s`
- Delta: `+0.217%` (V10 slower in that sample)
- Means: V9 `8.40837s`, V10 `8.38237s` (V10 better mean)

Interpretation:
- Fused correction pass is a real micro-improvement vs prior V10 build.
- V9/V10 ordering remains noise-sensitive; use longer alternating windows for claims.

### Extra follow-up (same pass): skip low-b correction indices (FAILED to show robust gain)

Attempt:
- In fused correction loop, skipped `bb <= c` (since D loops start at `b=c+1`).

7-pair A/B (fused baseline vs skip-low-b):
- baseline median: `8.38386s`
- skip-low-b median: `8.37972s` (tiny)
- means: baseline `8.38770s`, skip-low-b `8.40912s` (worse tails)

Verdict:
- Not robust; reverted.

## Continuation Pass: D Segmentation Floor Exploration + Additional Dead Ends

Date: 2026-03-01 (next pass)

### Kept code change (experimental knob only, default unchanged)

Added environment-controlled D minimum segment size floor:
- New env: `D_SEG_MIN_CAP` (default `17`)
- Segment-size formula now uses `max(..., 1 << D_SEG_MIN_CAP)` instead of hardcoded `1<<17`.
- Default behavior is unchanged when env is unset.

### Why this was explored

Timing output showed D using extremely high segment count at Max i64 (`~151k` segments), suggesting scheduler/segment overhead might still be tunable.

### Validation summary

#### 1) `D_SEG_MIN_CAP` sweeps

Single-run sweep (`D_SEG_CAP=20`):
- `17`: `8.67094s`
- `18`: `8.35847s`
- `19`: `8.35658s`
- `20`: `8.35580s`

5 alternating (`17` vs `20`):
- `17` median: `8.39155s`
- `20` median: `8.37483s`
- Means: `17` = `8.42336s`, `20` = `8.37263s`

11 alternating (`17` vs `20`):
- `17` median: `8.40966s`
- `20` median: `8.38189s` (better median)
- Means: `17` = `8.41504s`, `20` = `8.43839s` (worse tails)

11 alternating (`17` vs `19`):
- `17` median: `8.40669s`
- `19` median: `8.39389s`
- Means: `17` = `8.43183s`, `19` = `8.39440s`

15 alternating (`17` vs `19`):
- `17` median: `8.41047s`
- `19` median: `8.39287`
- Means: `17` = `8.42142s`, `19` = `8.39259s`

Interpretation:
- Higher min floor (`19`/`20`) can improve V10-vs-V10 medians.
- Tail behavior and cross-version comparisons are inconsistent; not robust enough to change defaults.
- Keep as an experimental knob; default remains `17`.

#### 2) Cross-checks vs V9

11 alternating, V10 with `D_SEG_MIN_CAP=19`:
- V9 median: `8.37376s`
- V10 median: `8.37547s` (essential tie/slightly slower median)
- Means: V9 `8.40025s`, V10 `8.37874s` (V10 better mean)

11 alternating, V10 default (`D_SEG_MIN_CAP=17`):
- V9 median: `8.41529s`
- V10 median: `8.39601s` (V10 faster)
- Means: V9 `8.42835s`, V10 `8.40010s`

Current practical stance:
- Keep default min floor at `17`.
- Use `D_SEG_MIN_CAP` only for targeted experimentation.

### Additional avenues tested (FAILED / reverted)

1. D uniform chunking (`D_UNIFORM_CHUNKING`) path:
- Catastrophic regressions (`~18s` to `~43s` depending on chunk count).
- Fully reverted.

2. D dedicated threads (`D_THREADS`) re-sweep:
- All tested values slower than default global-pool path.

3. C1 pool tuning (`C1_THREADS`) experiments:
- No robust, reproducible net gain over existing behavior.
- Reverted code changes from this path.

## Continuation Pass: Additional D/Phase Paths (No New Robust Default Win)

Date: 2026-03-02

### Attempt A: D chunk skew-stat gating (FAILED, reverted)

Hypothesis:
- When `D_ADAPT_CHUNKS=0`, `D_AUTO_CHUNK_SELECT=0`, and `SHOW_TIMING` is off,
  skip p95/max-skew sampling/sort overhead in D setup.

Result:
- Balanced A/B showed regression (combined 14-run view):
  - old median `8.42708s`
  - gated median `8.49168s`
  - delta `+0.767%`
- Reverted.

### Attempt B: D chunk pre-scan removal via lazy phi/coeff growth (FAILED, reverted)

Hypothesis:
- Remove per-chunk `chunk_max_b` pre-scan and grow vectors lazily during segment loop.

Result (7 alternating old vs new):
- old median `8.38853s`
- new median `8.44245s`
- delta `+0.643%`
- Reverted.

### Attempt C: Scheduler phase modes re-sweep (all slower)

Single-run checks (Max i64):
- `DEFAULT`: `8.38453s`
- `SEQ_MODE`: `9.24162s`
- `PHASE_DB_AC`: `9.16773s`
- `PHASE_AC_DB`: `8.90709s`
- `PHASE_D_ACB`: `8.99463s`
- `PHASE_D_ACB2`: `9.01771s`
- `PHASE_D_ACB3`: `15.75224s`
- `PHASE_B_ACD`: `9.33036s`

Conclusion:
- Keep default concurrent schedule.

### Attempt D: Expanded D grid around current defaults (inconclusive)

Grid sampled over:
- `D_SEG_MIN_CAP` in `{17, 19}`
- `D_ADAPT_CHUNKS` in `{0, 1}`
- `D_CHUNKS` in `{20, 24, 28}`

Best single-run candidate observed:
- `D_SEG_MIN_CAP=17`, `D_ADAPT_CHUNKS=1`, `D_CHUNKS=28` (`8.32698s` in one run)

11 alternating candidate vs base (`17/0/24`):
- base median `8.40270s`
- candidate median `8.40509s`
- delta `+0.028%` (no real median win)
- means slightly favor candidate, but too small/noisy for default change.

### Net after this pass

- No new robust default improvement beyond the current retained V10 baseline.
- Existing defaults remain:
  - `AUTO_TUNE=0`
  - `D_WAIT_MS=0`
  - `D_ADAPT_CHUNKS=0`
  - `D_CHUNKS=24`
  - `D_AUTO_CHUNK_SELECT=0`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=17` (new knob from prior pass; default unchanged)

## Breakthrough Continuation: AC Segment-Level Parallelism Gating

Date: 2026-03-02 (later)

### Implemented and Kept

Added AC segment-level serial fallback threshold:
- New env: `AC_PAR_MIN`
- In `compute_ac`, per-segment work now runs:
  - serial loop when `n_total < AC_PAR_MIN`
  - rayon parallel loop otherwise

Rationale:
- Avoid rayon dispatch overhead for small segment workloads while preserving full parallelism for larger segments.

### Tuning outcome

Initial sweep (single-run signal) over `AC_PAR_MIN`:
- `0`: `8.40103s`
- `128`: `8.42673s`
- `256`: `8.35825s` (best in sweep)
- `384`: `8.41536s`
- `512`: `8.40759s`
- `768`: `8.45688s`
- `1024`: `8.42710s`

Alternating checks:
- `256` vs `0` (5 pairs): tiny median edge to `256` (`-0.072%`), better mean.
- `256` vs `384` (5 pairs): clear win for `256`.

Default changed:
- `AC_PAR_MIN` fallback set to `256`.

### Validation vs previous V10 (before AC_PAR_MIN change)

11 alternating pairs, Max i64:
- old median: `8.41920s`
- new median: `8.38224s`
- Delta: `-0.439%`
- Means: old `8.45574s`, new `8.38826s`

### Cross-check vs V9 with new default

11 alternating pairs, Max i64:
- V9 median: `8.45668s`
- V10 median: `8.38859s`
- Delta: `-0.805%`
- Means: V9 `8.48238s`, V10 `8.39302s`

## Additional paths attempted in this continuation and rejected

1. D chunk pre-scan removal via lazy vector growth:
- Regressed (~`+0.64%` median), reverted.

2. D work-model sample-point reduction:
- Inconclusive/no robust gain, not kept.

3. D skew-stat gating when adapt/auto off:
- Regressed in balanced A/B, reverted.

4. Phase-mode re-sweep and expanded D grid:
- No robust default improvement.

## Continuation Correction: AC_PAR_MIN Retune After Longer Validation

Date: 2026-03-02 (latest)

### Context

Initial introduction of `AC_PAR_MIN` suggested `256` as a default in one run window.
A longer follow-up showed this was not robust under extended alternation.

### Additional validation

1. `AC_PAR_MIN=192` vs `256` (7 alternating):
- `192` median better by ~`0.76%`

2. `AC_PAR_MIN=192` vs `0` (11 alternating):
- `192` slightly worse median/mean than `0`

3. `AC_PAR_MIN=256` vs `0` (11 alternating):
- `256` worse median (`+0.249%`) and worse mean.

### Key cross-check

Compared current AC_PAR_MIN code (forcing `AC_PAR_MIN=0`) against pre-AC_PAR_MIN V10 from commit `c188bb3`:
- 11 alternating:
  - pre median: `8.46201s`
  - new (`AC_PAR_MIN=0`) median: `8.41724s`
  - Delta: `-0.529%`
  - Means also improved (`8.45810s` -> `8.41765s`)

Interpretation:
- The AC path refactor itself remains beneficial.
- Nonzero default thresholds were not stable enough.

### Final default decision

- Set `AC_PAR_MIN` default fallback to **0**.
- Keep `AC_PAR_MIN` knob for optional experimentation only.

### Latest V9 vs V10 (with new default)

11 alternating pairs:
- V9 median: `8.46338s`
- V10 median: `8.37627s`
- Delta: `-1.029%` (V10 faster)
- Means: V9 `8.46616s`, V10 `8.36693s`

## Continuation Pass: AC_PAR_MIN Stability Audit + AC_SEG/D_CHUNKS Retests

Date: 2026-03-03

### 1) AC_PAR_MIN follow-up (important correction)

Extended alternation showed prior `AC_PAR_MIN=256` default was not stable:
- `256` vs `0` (11 alternating):
  - `256` median slower by `+0.249%`
  - mean also worse.
- `192` vs `0` (11 alternating):
  - `192` also slightly worse.

Result:
- Keep feature, but default remains `AC_PAR_MIN=0` (already set in code).

Cross-check:
- Current code with `AC_PAR_MIN=0` still beats pre-AC_PAR_MIN V10 (`c188bb3`) in 11-pair A/B:
  - pre median `8.46201s`
  - new median `8.41724s`
  - delta `-0.529%`.

### 2) D skew-sample selection experiment (FAILED, reverted)

Tried replacing p95 sample full sort with `select_nth_unstable`.
- 11 alternating old vs new regressed strongly:
  - old median `8.37909s`
  - new median `8.43419s`
  - delta `+0.658%`.
- Reverted.

### 3) D work-sample size retune (`D_SKEW_SAMPLE`) (FAILED to hold up)

- Single runs hinted possible gains at some sample sizes.
- 7 and 11 alternating checks showed default-equivalent behavior was safer; candidate sizes regressed.
- No code change kept from this path.

### 4) Runtime retunes with current code

#### POOL_MULT
- 11 alternating `2` vs `3` confirmed `3` is decisively better.

#### D_SEG_CAP
- 11 alternating `19` vs `20` confirmed `20` decisively better.

#### D_CHUNKS
- 11 alternating `20` vs `24`: mixed/noisy signal.
- 15 alternating `20` vs `24`: tiny edge to `24` (median and mean).
- Keep default `24`.

#### AC_SEG
- Several runs showed `160k` competitive and sometimes faster than `180k`.
- 11 alternating `160k` vs `180k`: very small median edge to `160k`, tiny mean edge to `180k` (inconclusive).
- Vs V9, both `160k` and `180k` produced solid wins in different windows; no stable reason yet to change default.
- Keep default `180000`.

## Net after this pass

- No new retained code changes.
- This pass mainly hardened defaults by rejecting non-robust alternates.
- Current practical defaults unchanged:
  - `AC_PAR_MIN=0`
  - `AC_SEG=180000`
  - `D_SEG_CAP=20`
  - `D_CHUNKS=24`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: Deep B/D Retest + B_CHUNKS Default Retune

Date: 2026-03-03 (later)

### Failed code-path attempts (reverted)

1. `compute_b` chunk-boundary linear sweep (replace partition_point loop):
- 11 alternating old vs new:
  - old median `8.38425s`
  - new median `8.42425s`
  - Delta `+0.477%` (worse)
- Reverted.

2. D Type-2 monotonic `l` cap carryover (with and without improved init):
- Both variants regressed severely (one catastrophic ~13s).
- Reverted.

3. `D_SKEW_SAMPLE` tunable sample-size path:
- Single-run signals were inconsistent.
- 7/11 alternating checks did not support non-default sample sizes.
- No retained code/default change from this knob.

### Runtime retest highlights

- `POOL_MULT=3` reaffirmed strongly over `2`.
- `D_SEG_CAP=20` reaffirmed strongly over `19`.
- `D_CHUNKS`: long comparisons remained close/noisy (`20` vs `24` tiny).
- `AC_SEG`: `160k` looked competitive but not robust enough to replace `180k`.

### Retained default retune: `B_CHUNKS` 4 -> 2

Evidence:
- 11 alternating (`B_CHUNKS=2` vs `4`):
  - median improvement for `2`: about `-0.566%`
  - mean also improved.
- 15 alternating (`2` vs `4`):
  - median still better for `2` (`-0.129%`)
  - means essentially equal.

Decision:
- Change V10 default `B_CHUNKS` fallback from `4` to `2`.
- Keep env override behavior unchanged.

### Net

Current V10 defaults after this pass:
- `AC_PAR_MIN=0`
- `AC_SEG=180000`
- `B_CHUNKS=2`  (updated)
- `D_SEG_CAP=20`
- `D_CHUNKS=24`
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: AC_SEG Re-Tune (Robust Long-Window Win)

Date: 2026-03-03 (latest)

### Key finding

A longer controlled alternation finally produced a robust AC segment-size winner:
- `AC_SEG=160000` vs `180000` over **21 alternating pairs**:
  - median: `8.34402s` vs `8.38652s`
  - delta: `-0.507%`
  - means: `8.35640s` vs `8.39867s`

This is the first long-window result clearly favoring `160000` on both median and mean.

### Cross-check vs V9 (with AC_SEG=160000)

11 alternating pairs:
- V9 median: `8.42402s`
- V10 median: `8.38537s`
- delta: `-0.459%`
- means: V9 `8.44755s`, V10 `8.37008s`

### Retained default change

- Updated V10 default `AC_SEG` fallback:
  - `180000 -> 160000`

### Runtime auto-tuning sync

- Updated top runtime-tuning tier (`x >= 1e18`) to match current best defaults:
  - `ac_seg: 160000`
  - `b_chunks: 2`

### Additional attempts in this pass (not kept)

1. `compute_b` chunk-bound linear sweep:
- Regressed on median in 11-pair A/B; reverted.

2. D Type-2 monotonic l-cap variants:
- Regressed (one catastrophic); reverted.

3. D skew-sample/tuning experiments:
- No stable win; not retained.

4. AC_PAR_MIN fast-path branch removal for default 0:
- Near-noise and slightly worse median; reverted.

## Current practical defaults after this pass

- `AC_SEG=160000`  (updated)
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_CHUNKS=24`
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: D_SEG_MIN_CAP Retune (Robust Win)

Date: 2026-03-06

Compile/runtime setup used:
- nightly release build with `-Zbuild-std=std,panic_abort` on `x86_64-pc-windows-msvc`
- `.cargo/config.toml` rustflags unchanged (`target-cpu=native`, `--unroll-threshold=800`, `-Zmir-opt-level=4`, `-Ztune-cpu=arrowlake`)
- large pages enabled (`MIMALLOC_LARGE_OS_PAGES=1`)

### Revalidation around current baseline (no retained change)

1. `B_CHUNKS=4` vs `2` (22 alternating):
- `2` median `8.41552s`, mean `8.42074s`
- `4` median `8.42384s`, mean `8.42603s`
- `2` still slightly better; keep default `B_CHUNKS=2`.

2. `D_CHUNKS=20` vs `24` (22 alternating):
- `20` median `8.42866s`, mean `8.42937s`
- `24` median `8.41654s`, mean `8.44099s`
- split signal (median vs mean), no default change.

3. `D_AUTO_CHUNK_SELECT=1` vs `0` (22 alternating):
- `1` median `8.40148s`, mean `8.39201s`
- `0` median `8.39491s`, mean `8.41472s`
- mixed signal; keep default `0`.

### New retained default improvement

`D_SEG_MIN_CAP` sweep showed a new strong candidate at `14`.

22 alternating (`D_SEG_MIN_CAP=14` vs `17`) with current defaults:
- `14` median `8.37065s`, mean `8.37096s`
- `17` median `8.42967s`, mean `8.43422s`
- improvement for `14`:
  - median `-0.700%`
  - mean `-0.750%`

### V9 cross-check

22 alternating (`V10@cap14` vs `V9`):
- V10 median `8.43259s`, mean `8.41989s`
- V9 median `8.48835s`, mean `8.49885s`
- V10 deltas:
  - median `-0.657%`
  - mean `-0.929%`

### Retained change

- Updated V10 default `D_SEG_MIN_CAP` fallback:
  - `17 -> 14`

### Current practical defaults after this pass

- `AC_SEG=160000`
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`  (updated)
- `D_CHUNKS=24`
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: AC_SEG Re-Tune After D_SEG_MIN_CAP Update

Date: 2026-03-06 (later)

Compile/runtime setup unchanged:
- nightly + `-Zbuild-std=std,panic_abort`
- same rustflags and large-page setup as above

### D_SEG_MIN_CAP neighborhood check

Additional checks after promoting `D_SEG_MIN_CAP=14`:
- `14` vs `13` (22 alternating): `14` better on median/mean.
- `14` vs `15`:
  - one 22-run showed `15` better,
  - one 30-run reversed-order showed near-tie with tiny split signal.
- `15` vs `17` (22 alternating): `15` did not hold a win.

Decision:
- Keep `D_SEG_MIN_CAP=14` as the retained default.

### AC re-tune with `D_SEG_MIN_CAP=14`

22 alternating (`AC_SEG=170000` vs `160000`):
- `170000` median `8.42881s`, mean `8.40599s`
- `160000` median `8.43594s`, mean `8.44978s`
- delta: median `-0.085%`, mean `-0.518%`

30 alternating reversed-order (`160000` vs `170000`):
- `160000` median `8.44012s`, mean `8.45750s`
- `170000` median `8.42545s`, mean `8.42223s`
- delta (`170000` vs `160000`): median `-0.174%`, mean `-0.417%`

### V9 cross-check (fully tuned V10)

22 alternating (`V10 tuned` vs `V9`):
- V10 median `8.42553s`, mean `8.41991s`
- V9 median `8.48092s`, mean `8.49638s`
- V10 deltas:
  - median `-0.653%`
  - mean `-0.900%`

### Retained changes

1. `D_SEG_MIN_CAP` default fallback:
- `17 -> 14`

2. `AC_SEG` default fallback:
- `160000 -> 170000`

3. Runtime top-tier tuning sync (`x >= 1e18`):
- `ac_seg: 170000`

### Current practical defaults after this pass

- `AC_SEG=170000`  (updated)
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`
- `D_CHUNKS=24`
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: Post-Retune Exhaustion Sweep

Date: 2026-03-06 (latest)

Setup: same nightly build + large-page runtime conditions.

### Knob rechecks (no retained default changes)

1. `B_CHUNKS=1` vs `2` (22 alternating, with `AC_SEG=170000`, `D_SEG_MIN_CAP=14`):
- `1` median `8.41581s`, mean `8.40010s`
- `2` median `8.41579s`, mean `8.38957s`
- essentially tied on median; mean favors `2` slightly. Keep `B_CHUNKS=2`.

2. `AC_PAR_MIN=192` vs `0` (22 alternating):
- `192` median `8.42398s`, mean `8.40171s`
- `0` median `8.40899s`, mean `8.36616s`
- nonzero threshold regressed; keep `AC_PAR_MIN=0`.

3. Single-run sweeps under current defaults:
- `D_SEG_CAP`: `20` remained clear optimum (18/19/21/22 slower).
- `D_CHUNKS`: `24` remained best among 16/20/24/28/32.
- `POOL_MULT`: `3` remained best vs `2` and `4`.

### Code-path experiments (FAILED, reverted)

1. `compute_b` pre-sum fast path (`pi_fast` + unchecked for `x/p <= sqrt(x)`):
- A/B via env toggle over 22 alternating runs:
  - fast variant median/mean worse (`+0.168%` / `+0.050%`).
- Reverted.

2. AC crossover scalarization in unrolled mixed-boundary case:
- A/B via env toggle over 22 alternating runs:
  - scalar variant median/mean worse (`+0.123%` / `+0.166%`).
- Reverted.

### Net

- No additional retained wins in this continuation.
- Current tuned defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: ValidM Sparse-Index Retune

Date: 2026-03-06 (latest later)

Setup unchanged:
- nightly + `-Zbuild-std=std,panic_abort`
- large pages enabled
- tuned baseline before this pass:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `POOL_MULT=3`

### New knob under test

- Exposed `VM_STRIDE` for the sparse `ValidM` index used by D Type 1 range lookup.
- Prior default behavior corresponded to `VM_STRIDE=64`.

### Sweep

Single-run sweep:
- `16`: `8.36899s`
- `32`: `8.32493s`
- `48`: `8.38073s`
- `64`: `8.34255s`
- `96`: `8.37201s`
- `128`: `8.44104s`
- `192`: `8.43260s`

Neighbor check:
- `24`: `8.33934s`
- `32`: `8.40705s`
- `40`: `8.59236s`

### Alternating validation

1. `32` vs `64` (22 alternating):
- `32` median `8.38560s`, mean `8.38923s`
- `64` median `8.42876s`, mean `8.43115s`
- delta (`32` vs `64`):
  - median `-0.512%`
  - mean `-0.497%`

2. `24` vs `64` (22 alternating):
- `24` median `8.39720s`, mean `8.38703s`
- `64` median `8.46429s`, mean `8.44584s`
- delta (`24` vs `64`):
  - median `-0.793%`
  - mean `-0.696%`

3. `24` vs `32`:
- 22 alternating gave mixed signal.
- 30 alternating, reversed order:
  - `32` median `8.39972s`, mean `8.38068s`
  - `24` median `8.40756s`, mean `8.40000s`
  - delta (`32` vs `24`):
    - median `-0.093%`
    - mean `-0.230%`

Decision:
- Keep `VM_STRIDE=32` as the new default.
- `24` is competitive, but `32` won the longer direct comparison and already had a clean win over the prior default.

### V9 cross-check

22 alternating (`V10 tuned + VM_STRIDE=32` vs `V9`):
- V10 median `8.36997s`, mean `8.35495s`
- V9 median `8.48300s`, mean `8.49954s`
- V10 deltas:
  - median `-1.332%`
  - mean `-1.701%`

### Retained change

- Updated default sparse index stride:
  - `VM_STRIDE: 64 -> 32`

### Current practical defaults after this pass

- `AC_SEG=170000`
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`
- `D_CHUNKS=24`
- `VM_STRIDE=32`  (updated)
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

### Follow-up on sparse-index search window (not retained)

After promoting `VM_STRIDE=32`, exposed `VM_LOOKAHEAD` to test the search window width used with `vm_index`.

Single-run sweep at `VM_STRIDE=32`:
- `1`: `8.37509s`
- `2`: `8.33833s`
- `3`: `8.47378s`
- `4`: `8.44703s`

22 alternating (`VM_LOOKAHEAD=1` vs `2`):
- `1` median `8.41379s`, mean `8.39595s`
- `2` median `8.43287s`, mean `8.38816s`
- mixed signal (median favors `1`, mean favors `2`)

Decision:
- Keep default `VM_LOOKAHEAD=2`.
- Leave knob available for future experiments, but do not promote a default change.

## Continuation Pass: Post-VM_STRIDE Revalidation

Date: 2026-03-06 (latest final)

### VM_STRIDE neighborhood check

Single-run neighborhood sweep:
- `20`: `8.32340s`
- `24`: `8.36907s`
- `28`: `8.36175s`
- `32`: `8.26248s`
- `36`: `8.37108s`

This reaffirmed that the retained `32` default is still the best local point.

### AC_SEG retune after VM_STRIDE update (not retained)

Single-run sweep:
- `150000`: `8.34775s`
- `160000`: `8.48355s`
- `170000`: `8.38220s`
- `180000`: `8.30022s`
- `190000`: `8.44112s`

Long checks:

1. `180000` vs `170000` (22 alternating):
- `180000` median `8.41265s`, mean `8.43212s`
- `170000` median `8.38373s`, mean `8.38214s`
- `180000` regressed on both median and mean.

2. `150000` vs `170000` (22 alternating):
- `150000` median `8.40437s`, mean `8.38331s`
- `170000` median `8.40029s`, mean `8.37788s`
- essentially flat, with a slight edge to `170000`.

Decision:
- Keep `AC_SEG=170000`.

### CompactPi stride probe (compile-time, not retained)

Quick compile-time checks:
- `PI_STRIDE=128`: `8.41046s` single run (worse)
- `PI_STRIDE=512`: `8.30897s` single run

Interpretation:
- `128` is clearly worse.
- `512` looked interesting in a single run, but there was not enough evidence to justify changing a core compile-time layout that was previously retained at `256`.
- Reverted to `PI_STRIDE=256`.

### Net

- No additional retained default/code change in this continuation beyond the earlier `VM_STRIDE=32`.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=32`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: VM_LOOKAHEAD Long-Window Resolution

Date: 2026-03-07

After the earlier mixed 22-run result for `VM_LOOKAHEAD=1` vs `2`, reran the comparison with reversed order and a longer window.

30 alternating, reversed order (`2` vs `1`) with `VM_STRIDE=32`:
- `2` median `8.39837s`, mean `8.39464s`
- `1` median `8.38776s`, mean `8.37180s`
- delta (`1` vs `2`):
  - median `-0.126%`
  - mean `-0.272%`

### V9 cross-check

22 alternating (`V10 tuned + VM_LOOKAHEAD=1` vs `V9`):
- V10 median `8.38692s`, mean `8.36678s`
- V9 median `8.44541s`, mean `8.46209s`
- V10 deltas:
  - median `-0.693%`
  - mean `-1.126%`

### Retained change

- Updated default sparse-index search lookahead:
  - `VM_LOOKAHEAD: 2 -> 1`

### Current practical defaults after this pass

- `AC_SEG=170000`
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`
- `D_CHUNKS=24`
- `VM_STRIDE=32`
- `VM_LOOKAHEAD=1`  (updated)
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: VM_STRIDE Re-Tune Under VM_LOOKAHEAD=1

Date: 2026-03-07 (later)

With `VM_LOOKAHEAD=1` retained, reran the stride search because the optimum could shift.

### Sweep

Single-run neighborhood sweep:
- `20`: `8.48676s`
- `24`: `8.50167s`
- `28`: `8.49505s`
- `32`: `8.71885s`
- `36`: `8.52388s`

This sweep was noisy, so it was not used for the decision.

### Direct comparisons

1. `24` vs `32` (22 alternating):
- `24` median `8.43394s`, mean `8.41505s`
- `32` median `8.44779s`, mean `8.41484s`
- small median edge for `24`, means effectively tied.

2. `32` vs `24` (30 alternating, reversed order):
- `32` median `8.42132s`, mean `8.40215s`
- `24` median `8.38373s`, mean `8.37190s`
- delta (`24` vs `32`):
  - median `-0.446%`
  - mean `-0.360%`

### V9 cross-check

22 alternating (`V10 tuned + VM_STRIDE=24 + VM_LOOKAHEAD=1` vs `V9`):
- V10 median `8.41515s`, mean `8.39574s`
- V9 median `8.48044s`, mean `8.46294s`
- V10 deltas:
  - median `-0.770%`
  - mean `-0.794%`

### Retained change

- Updated default sparse index stride:
  - `VM_STRIDE: 32 -> 24`

### Current practical defaults after this pass

- `AC_SEG=170000`
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`
- `D_CHUNKS=24`
- `VM_STRIDE=24`  (updated)
- `VM_LOOKAHEAD=1`
- `D_ADAPT_CHUNKS=0`
- `D_AUTO_CHUNK_SELECT=0`
- `POOL_MULT=3`

## Continuation Pass: Post-VM_STRIDE24 Exhaustion Sweep

Date: 2026-03-07 (latest)

Baseline for this pass:
- `AC_SEG=170000`
- `AC_PAR_MIN=0`
- `B_CHUNKS=2`
- `D_SEG_CAP=20`
- `D_SEG_MIN_CAP=14`
- `D_CHUNKS=24`
- `VM_STRIDE=24`
- `VM_LOOKAHEAD=1`
- `POOL_MULT=3`

### Sparse-index follow-ups (no retained change)

1. `VM_LOOKAHEAD` recheck under `VM_STRIDE=24`

Single-run sweep:
- `1`: `8.33490s`
- `2`: `8.41273s`
- `3`: `8.38188s`
- `4`: `8.29308s`

22 alternating (`4` vs `1`):
- `4` median `8.38796s`, mean `8.39807s`
- `1` median `8.38414s`, mean `8.41183s`
- mixed signal; keep `VM_LOOKAHEAD=1`.

2. Joint `(VM_STRIDE, VM_LOOKAHEAD)` candidates

22 alternating (`32,2` vs current `24,1`):
- `32,2` median `8.39765s`, mean `8.39204s`
- `24,1` median `8.42470s`, mean `8.40996s`
- short-window signal favored `32,2`.

30 alternating, reversed order (`24,1` vs `32,2`):
- `24,1` median `8.37864s`, mean `8.37609s`
- `32,2` median `8.39466s`, mean `8.38760s`
- longer-window result favored `24,1`.

22 alternating (`16,2` vs current `24,1`):
- `16,2` median `8.39087s`, mean `8.39059s`
- `24,1` median `8.40930s`, mean `8.37984s`
- mixed signal; not enough to replace current defaults.

Decision:
- Keep `VM_STRIDE=24`, `VM_LOOKAHEAD=1`.

### D scheduler rechecks (no retained change)

1. `D_SEG_MIN_CAP`
- Single-run sweep favored `10` and `16`, but long checks did not support changing the default:
  - `16` vs `14` (22 alternating): `16` worse on median and mean.
  - `10` vs `14` (22 alternating): median favored `14`, mean favored `10` slightly; mixed.

2. `D_SEG_CAP` single-run recheck:
- `19`: `8.37827s`
- `20`: `8.30812s`
- `21`: `8.80946s`
- keep `20`.

3. `D_CHUNKS` single-run recheck:
- `20`: `8.41830s`
- `24`: `8.36003s`
- `28`: `8.44006s`
- keep `24`.

4. `B_CHUNKS` single-run recheck:
- `1`: `8.55555s`
- `2`: `8.31342s`
- `3`: `8.45971s`
- `4`: `8.51729s`
- keep `2`.

### Net

- No additional retained default/code change in this continuation.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: Joint Sparse-Index Surface + Search-Array Probe

Date: 2026-03-07 (latest later)

### Joint sparse-index retune (no retained change)

Rechecked the combined `(VM_STRIDE, VM_LOOKAHEAD)` surface from the current `24/1` baseline.

1. `VM_LOOKAHEAD` sweep at `VM_STRIDE=24`:
- `1`: `8.33490s`
- `2`: `8.41273s`
- `3`: `8.38188s`
- `4`: `8.29308s`

22 alternating (`4` vs `1`):
- `4` median `8.38796s`, mean `8.39807s`
- `1` median `8.38414s`, mean `8.41183s`
- mixed signal; no change.

2. `(32,2)` vs current `(24,1)`:
- 22 alternating initially favored `(32,2)` on both median and mean.
- 30 alternating, reversed order, favored `(24,1)`:
  - `(24,1)` median `8.37864s`, mean `8.37609s`
  - `(32,2)` median `8.39466s`, mean `8.38760s`

3. `(16,2)` vs current `(24,1)` (22 alternating):
- `(16,2)` median `8.39087s`, mean `8.39059s`
- `(24,1)` median `8.40930s`, mean `8.37984s`
- mixed signal; no change.

Decision:
- Keep `VM_STRIDE=24`, `VM_LOOKAHEAD=1`.

### Code experiment: split `m` search array (FAILED, reverted)

Hypothesis:
- Keep `ValidM` AoS layout for the actual Type 1 loop, but search bucket bounds using a separate `Vec<u32>` of `m` values to avoid touching 16-byte records during `partition_point`.

22 alternating (`VM_VALUE_SEARCH=1` vs `0`):
- `1` median `8.44256s`, mean `8.42447s`
- `0` median `8.42246s`, mean `8.41628s`
- regression on both median and mean.

Decision:
- Reverted the split-search-array change.

### Net

- No retained code/default change from this continuation.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: D Same-Bucket Sparse-Index Fast Path

Date: 2026-03-11

### Code experiment: monotone Type 1 bucket window (`VM_MONO_WINDOW`) (FAILED, reverted)

Hypothesis:
- Exploit the monotonic shrinkage of Type 1 `vm_start` / `vm_end` across increasing `b`
  by capping the next search window to the previous range.

22 alternating (`VM_MONO_WINDOW=1` vs `0`):
- `1` median `8.50707s`, mean `8.50628s`
- `0` median `8.51046s`, mean `8.49632s`
- mixed and slightly worse on mean.

Decision:
- Reverted.

### Code experiment: local `phi[b]` / `coeff[b]` accumulators (FAILED, reverted)

Hypothesis:
- Hoist `phi[b]` into a register-local `phi_b` and accumulate `coeff[b]` deltas locally so
  the Type 1 / Type 2 leaf loops write back once per `b` instead of on every hit.

Result:
- Correct in `SEQ_MODE=1`.
- Correct with dedicated D pools (`D_THREADS=1` and `D_THREADS=2`).
- Incorrect in the default concurrent/global-pool flow at `Max i64`:
  - got `39168468951557104`
  - expected `216289611853439384`

Decision:
- Reverted immediately. The path is not safe enough to pursue further without deeper race/UB analysis.

### Retained code change: same-bucket sparse-index fast path

Hypothesis:
- When Type 1 `min_m` and `max_m` land in the same `vm_index` bucket, reuse the same
  `ValidM` search slice for both `partition_point` calls instead of rebuilding an equivalent
  search window twice.

Implementation:
- Added a same-bucket fast path in `compute_d`.
- If `min_bucket == max_bucket`, the code now computes both `vm_start` and `vm_end`
  from the same `hint..search_end` slice.
- Cross-bucket behavior is unchanged.

22 alternating, candidate first:
- candidate median `8.33582s`, mean `8.33527s`
- baseline median `8.36631s`, mean `8.36270s`
- delta `-0.364%` median, `-0.328%` mean

30 alternating, reversed order:
- candidate median `8.34536s`, mean `8.34760s`
- baseline median `8.37685s`, mean `8.36912s`
- delta `-0.376%` median, `-0.257%` mean

### V9 cross-check

22 alternating:
- V10 median `8.33711s`, mean `8.33456s`
- V9 median `8.45212s`, mean `8.44869s`
- delta `-1.361%` median, `-1.351%` mean

Decision:
- Keep the same-bucket fast path.

### Net

- Retained one new code-level D improvement in this continuation.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: Post Same-Bucket Refinement Sweep

Date: 2026-03-11 (later)

### Compile/profile parity recheck

Revalidated that V10 is already using the documented V8/V9 compile-time platform settings:
- nightly release build with `-Zbuild-std=std,panic_abort`
- `.cargo/config.toml` rustflags:
  - `-C target-cpu=native`
  - `-C llvm-args=--unroll-threshold=800`
  - `-Zmir-opt-level=4`
  - `-Ztune-cpu=arrowlake`
- `Cargo.toml` release profile still uses `lto="fat"` and `codegen-units=1`

Conclusion:
- No new compile-time lever remains locally unexhausted in the repo configuration.

### Code experiment: same-bucket tail search refinement (FAILED, reverted)

Hypothesis:
- In the retained same-bucket path, compute `vm_end` by starting the second `partition_point`
  from `vm_start_rel` instead of rescanning the full bucket slice.

22 alternating (`tail-search` vs retained same-bucket baseline):
- candidate median `8.38658s`, mean `8.38811s`
- baseline median `8.35835s`, mean `8.36344s`
- delta `+0.338%` median, `+0.295%` mean

Decision:
- Reverted.

### Code experiment: unchecked bucket-slice helper (FAILED, reverted)

Hypothesis:
- `vm_index` entries are already guaranteed in-bounds, so remove the redundant
  clamp logic and use a small helper with unchecked bucket access / slice creation.

22 alternating:
- candidate median `8.38352s`, mean `8.40733s`
- baseline median `8.34043s`, mean `8.33614s`
- delta `+0.517%` median, `+0.854%` mean

Decision:
- Reverted.

### Code experiment: manual reverse `ValidM` walk (FAILED, reverted)

Hypothesis:
- Replace `iter().rev()` in Type 1 with a manual reverse index walk over the
  `ValidM` slice to reduce iterator overhead in the leaf loop.

22 alternating:
- candidate median `8.48688s`, mean `8.47236s`
- baseline median `8.36883s`, mean `8.36710s`
- delta `+1.411%` median, `+1.258%` mean

Decision:
- Reverted.

### Net

- No additional retained code/default change from this continuation.
- The retained same-bucket sparse-index fast path stands.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: D Sparse-Index Instrumentation

Date: 2026-03-11 (instrumented continuation)

### Retained diagnostic change

- Added env-gated `D_VM_STATS=1` instrumentation inside `compute_d()`.
- The instrumentation prints:
  - total Type 1 sparse-index query count
  - same-bucket vs cross-bucket share
  - empty-result share
  - bucket-delta histogram
  - average searched-item and result-item counts for same-bucket and cross-bucket lookups
- Default behavior is unchanged when `D_VM_STATS` is unset.

### Measurement: Max i64

Single instrumentation run (`D_VM_STATS=1`, `SHOW_TIMING=1`):
- queries `89,722,933`
- same-bucket `64,066,756` (`71.4%`)
- cross-bucket `25,656,177` (`28.6%`)
- empty-result queries `44,983,257` (`50.1%`)
- avg bucket delta `23.28`
- bucket delta hist:
  - `[0]=64,066,756`
  - `[1]=14,069,291`
  - `[2]=2,733,784`
  - `[3-4]=2,319,473`
  - `[5-8]=1,829,465`
  - `[9+]=4,704,164`
- same-bucket avg search items `4.1`, avg result items `0.4`
- cross-bucket avg start items `4.1`, avg end items `4.1`, avg result items `297.5`

Interpretation:
- The retained same-bucket path is already operating on very small search windows.
- Further same-bucket lookup surgery is unlikely to pay unless it changes more than a few comparisons.
- The only plausible remaining sparse-index specialization space is near-cross-bucket handling, but it needs data-backed validation because the actual leaf span can still be large.

### Code experiment: combined near-cross-bucket search window (FAILED, reverted)

Hypothesis:
- For cross-bucket lookups with bucket delta `<= 2`, replace the two separate
  bucket-slice probes with one contiguous `ValidM` search window.

22 alternating, candidate first:
- candidate median `8.39787s`, mean `8.39620s`
- baseline median `8.43345s`, mean `8.41562s`
- delta `-0.422%` median, `-0.231%` mean

22 alternating, reversed order:
- candidate median `8.43117s`, mean `8.41043s`
- baseline median `8.37713s`, mean `8.37719s`
- delta `+0.645%` median, `+0.397%` mean

Decision:
- Reverted.
- The result is too order-sensitive to treat as a production V10 gain.

### Net

- Retained the `D_VM_STATS` diagnostic hook only.
- No production-path/default change from this continuation.
- Current best V10 remains the retained same-bucket sparse-index fast path plus existing defaults.

## Continuation Pass: D Leaf-Loop Hoisting

Date: 2026-03-11 (later)

### Quick `VM_STRIDE` recheck under the retained same-bucket fast path

Rationale:
- The same-bucket fast path changed the lookup cost model, so larger buckets might have become
  favorable by sending more queries down the retained same-bucket path.

Single-run sweep (`VM_LOOKAHEAD=1` unchanged):
- `VM_STRIDE=16`: `8.45928s`
- `VM_STRIDE=24`: `8.36692s`
- `VM_STRIDE=32`: `8.54630s`
- `VM_STRIDE=40`: `8.42807s`
- `VM_STRIDE=48`: `8.38450s`
- `VM_STRIDE=64`: `8.42479s`

Decision:
- Keep `VM_STRIDE=24`.
- The current default still wins even after the same-bucket code-path change.

### Retained code change: hoist stable per-`b` values in D Type 1 / Type 2 loops

Hypothesis:
- After the new instrumentation showed that cross-bucket leaf spans dominate the remaining D work,
  reduce repeated hot-loop reloads by hoisting stable per-`b` values:
  - `phi[b]` into a local `phi_b`
  - `coeff[b]` into a single mutable slot reference per `b`
  - repeated `prime as usize` conversions
  - Type 1 `lpf` comparison to `u16`

Implementation:
- Hoisted those stable values in both Type 1 and Type 2 inside `compute_d()`.
- Semantics are unchanged; this is a codegen/locality improvement, not a new algorithm.

22 alternating, candidate first:
- candidate median `8.34957s`, mean `8.33941s`
- baseline median `8.41539s`, mean `8.41553s`
- delta `-0.782%` median, `-0.905%` mean

22 alternating, reversed order:
- candidate median `8.36884s`, mean `8.35786s`
- baseline median `8.42099s`, mean `8.40487s`
- delta `-0.619%` median, `-0.559%` mean

### V9 cross-check

22 alternating:
- V10 median `8.32575s`, mean `8.32448s`
- V9 median `8.40398s`, mean `8.40310s`
- delta `-0.931%` median, `-0.936%` mean

Decision:
- Keep the leaf-loop hoisting change.

### Net

- Retained one new V10 code-path improvement in D.
- Current retained defaults remain:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=2`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## Continuation Pass: Post-Push B Retune + Cleanup Sweep

Date: 2026-03-11 (post-push continuation)

### Rejected code experiments

#### 1) Type 2 `primes[l]` / `prime_recip[l]` local hoist

Hypothesis:
- Mirror the successful D leaf-loop hoist pattern inside Type 2 by loading
  `primes[l]` and `prime_recip[l]` once per iteration and turning the
  loop-condition boundary check into an in-loop break.

Result:
- 22 alternating vs pushed baseline:
  - candidate median `8.34966s`, mean `8.36464s`
  - baseline median `8.37725s`, mean `8.36474s`
  - delta `-0.038%` median, `-0.001%` mean
- Reversed order:
  - candidate median `8.35194s`, mean `8.35291s`
  - baseline median `8.33250s`, mean `8.33455s`
  - delta `+0.233%` median, `+0.220%` mean

Decision:
- Reverted.

#### 2) Cross-bucket sparse-index de-duplication

Hypothesis:
- Remove duplicated recomputation of the cross-bucket start/end sparse-index
  bounds that remained after landing `D_VM_STATS`.

Result:
- 22 alternating vs pushed baseline:
  - candidate median `8.38810s`, mean `8.36789s`
  - baseline median `8.39126s`, mean `8.36984s`
  - delta `-0.038%` median, `-0.023%` mean

Decision:
- Reverted as noise.

#### 3) AC C2/A loop specialization split

Hypothesis:
- Split the AC hot loop into separate `C2` and `A` inner loops so the unrolled
  path stops checking `info.is_c2` and recasting `b_term` on every batch.

Result:
- First Max-i64 validation run regressed catastrophically to `8.94187s`.

Decision:
- Reverted immediately.

### Knob rechecks on the pushed leaf-loop baseline

#### `D_CHUNKS`

Single-run sweep:
- `12`: `8.43595s`
- `16`: `8.31428s`
- `20`: `8.31859s`
- `24`: `8.29819s`
- `28`: `8.33694s`
- `32`: `8.42242s`

Decision:
- Keep `D_CHUNKS=24`.

#### `D_SEG_MIN_CAP`

Single-run sweep:
- `12`: `8.33185s`
- `13`: `8.31676s`
- `14`: `8.33067s`
- `15`: `8.33349s`
- `16`: `8.30284s`

22 alternating (`16` vs default `14`):
- candidate median `8.36279s`, mean `8.35904s`
- baseline median `8.34922s`, mean `8.34893s`
- delta `+0.163%` median, `+0.121%` mean

Decision:
- Keep `D_SEG_MIN_CAP=14`.

### Retained default retune: `B_CHUNKS` 2 -> 4

Single-run sweep:
- `1`: `8.33108s`
- `2`: `8.46658s`
- `3`: `8.37540s`
- `4`: `8.31649s`

22 alternating (`4` vs `2`), candidate first:
- candidate median `8.35652s`, mean `8.35615s`
- baseline median `8.37272s`, mean `8.37168s`
- delta `-0.193%` median, `-0.186%` mean

22 alternating, reversed order:
- candidate median `8.36472s`, mean `8.37107s`
- baseline median `8.37772s`, mean `8.36929s`
- delta `-0.155%` median, `+0.021%` mean

30 alternating, candidate first:
- candidate median `8.38674s`, mean `8.37154s`
- baseline median `8.39851s`, mean `8.40481s`
- delta `-0.140%` median, `-0.396%` mean

### V9 cross-check

22 alternating:
- V10 median `8.39108s`, mean `8.37955s`
- V9 median `8.43632s`, mean `8.42580s`
- delta `-0.536%` median, `-0.549%` mean

Decision:
- Update V10 default `B_CHUNKS` fallback from `2` to `4`.
- Sync the top auto-tune tier (`x >= 1e18`) to `b_chunks=4`.

### Net

- Retained one new default change from this continuation:
  - `B_CHUNKS: 2 -> 4`
- Current retained defaults are now:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=4`  (updated)
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## 2026-03-13 - D leaf interval-check pruning

Hypothesis:
- The D Type 1 and Type 2 leaf windows already guarantee `low <= x/p/m < high`
  and monotonic nondecreasing wheel positions, so the hot loops were still paying
  for interval/order branches that never carried real work.

Change:
- In both parallel and serial D paths:
  - replaced `xpm > low && xpm < high` / `xpq > low && xpq < high` with the
    cheaper `!= low` split
  - removed the impossible `xpq >= high` break arm
  - simplified Type 2 running-count dispatch to `None / same-pos / delta`

Validation:
- Rebuilt `prime_count_v10` with nightly + `-Zbuild-std=std,panic_abort`.
- Full built-in correctness sweep still passed through `Max i64`.
- Instrumented single-run check:
  - baseline `D: 5.95s`
  - candidate `D: 5.91s`

12-run alternating A/B (`Max i64`, both orders, 6 candidate + 6 baseline):
- candidate median `8.34409s`, mean `8.34551s`
- baseline median `8.36555s`, mean `8.36550s`
- delta `-0.257%` median, `-0.239%` mean

Decision:
- Retained.

### Net

- Retained one new code win from this continuation:
  - prune redundant D leaf interval/order checks
- Current retained defaults are unchanged:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=4`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=24`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## 2026-03-13 - post-pruning retune and helper recheck

Starting point:
- Continued from the retained D branch-pruning baseline.
- Goal: verify whether the new baseline had shifted any local optima before
  opening another larger D rewrite.

### Rejected code experiment: unrolled `BitSieve` popcount helper

Hypothesis:
- Share a 4-way unrolled `popcnt_range()` helper between `count()` and
  `count_delta()` to reduce word-scan loop overhead in the D leaf path.

Result:
- First full validation sweep regressed badly:
  - `Max i64`: `9.22871s`

Decision:
- Reverted immediately.

### Knob rechecks on the branch-pruned baseline

#### `D_CHUNKS`

Single-run sweep:
- `24`: `8.33213s`
- `20`: `8.37925s`
- `28`: `8.34727s`
- `16`: `8.37477s`
- `32`: `8.39976s`
- `12`: `8.44201s`

Decision:
- Keep `D_CHUNKS=24`.

#### `B_CHUNKS`

Single-run sweep:
- `4`: `8.29420s`
- `3`: `8.38515s`
- `5`: `8.41830s`
- `2`: `8.29829s`
- `6`: `8.34366s`
- `1`: `8.37014s`

Decision:
- Keep `B_CHUNKS=4`.

#### `AC_SEG`

Single-run sweep:
- `170000`: `8.41417s`
- `160000`: `8.37445s`
- `180000`: `8.34739s`
- `150000`: `8.25136s`
- `190000`: `8.30056s`

12 alternating (`150000` vs default `170000`), both orders:
- candidate median `8.33961s`, mean `8.36056s`
- baseline median `8.31212s`, mean `8.31991s`
- delta `+0.331%` median, `+0.489%` mean

Decision:
- Keep `AC_SEG=170000`.

#### `VM_LOOKAHEAD`

Single-run sweep:
- `1`: `8.35458s`
- `2`: `8.34327s`
- `3`: `8.33634s`
- `4`: `8.36780s`
- `6`: `8.42623s`
- `8`: `8.46293s`

12 alternating (`3` vs default `1`), both orders:
- candidate median `8.33451s`, mean `8.34947s`
- baseline median `8.31745s`, mean `8.32553s`
- delta `+0.205%` median, `+0.288%` mean

Decision:
- Keep `VM_LOOKAHEAD=1`.

### Retained default retune: `VM_STRIDE` 24 -> 16

Single-run sweep:
- `24`: `8.36238s`
- `20`: `8.43590s`
- `28`: `8.35071s`
- `32`: `8.31691s`
- `16`: `8.30277s`
- `40`: `8.33418s`

12 alternating (`16` vs default `24`), both orders:
- candidate median `8.35757s`, mean `8.37164s`
- baseline median `8.41135s`, mean `8.40801s`
- delta `-0.639%` median, `-0.433%` mean

12 alternating (`16` vs `32`), both orders:
- `16` median `8.34854s`, mean `8.34400s`
- `32` median `8.36618s`, mean `8.37300s`
- delta `-0.211%` median, `-0.346%` mean in favor of `16`

Decision:
- Update V10 default `VM_STRIDE` fallback from `24` to `16`.

### Net

- Retained one new default change from this continuation:
  - `VM_STRIDE: 24 -> 16`
- Current retained defaults are now:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=0`
  - `B_CHUNKS=4`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=16`  (updated)
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## 2026-03-13 - post-`VM_STRIDE` follow-up and AC threshold retune

Starting point:
- Continued from the retained `VM_STRIDE=16` baseline.
- First goal was to see whether the stride change had also shifted the nearby
  sparse-index tuning surface before touching another code path.

### Sparse-index recheck (no retained change)

`(VM_STRIDE, VM_LOOKAHEAD)` single-run matrix:
- `(12,1)`: `8.25396s`
- `(12,2)`: `8.34972s`
- `(12,3)`: `8.62926s`
- `(16,1)`: `8.36784s`
- `(16,2)`: `8.37094s`
- `(16,3)`: `8.40311s`
- `(20,1)`: `8.37476s`
- `(20,2)`: `8.40959s`
- `(20,3)`: `8.39010s`
- `(24,1)`: `8.33794s`
- `(24,2)`: `8.35674s`
- `(24,3)`: `8.36953s`
- `(32,1)`: `8.31759s`
- `(32,2)`: `8.41087s`
- `(32,3)`: `8.35218s`

Read:
- The first matrix made `(12,1)` look unusually strong, but the follow-up
  singles around `10/12/14/16` were noisy enough that another direct default
  move was not justified yet.

Round-robin at `VM_LOOKAHEAD=1` (`10/12/14/16`, mixed order, 4 runs each):
- `10`: median `8.34921s`, mean `8.35482s`
- `12`: median `8.34999s`, mean `8.35183s`
- `14`: median `8.34133s`, mean `8.35084s`
- `16`: median `8.34481s`, mean `8.34261s`

Decision:
- Keep `VM_STRIDE=16`, `VM_LOOKAHEAD=1`.

### Retained default retune: `AC_PAR_MIN` 0 -> 32

Why reopen it:
- AC remained the longest phase in `SHOW_TIMING`, even after the recent D-side
  wins.
- `AC_PAR_MIN` is a local runtime threshold, so it is cheaper and safer to
  retune than another AC structural rewrite.

Initial recheck:
- `0`: `8.31446s`
- `64`: `8.28690s`
- `128`: `8.34878s`
- `256`: `8.34247s`
- `512`: `8.39430s`

Balanced 12 alternating (`64` vs default `0`):
- candidate median `8.35089s`, mean `8.35881s`
- baseline median `8.36044s`, mean `8.36578s`
- small but real edge for `64`

Nearby recheck:
- `0`: `8.31514s`
- `32`: `8.29343s`
- `48`: `8.35175s`
- `64`: `8.43150s`
- `96`: `8.30300s`
- `128`: `8.33690s`

Balanced 12 alternating (`32` vs default `0`):
- candidate median `8.32017s`, mean `8.31956s`
- baseline median `8.34767s`, mean `8.35889s`
- delta `-0.329%` median, `-0.470%` mean

Head-to-head (`32` vs `64`):
- First 12 alternating:
  - `32` median `8.33372s`, mean `8.35545s`
  - `64` median `8.35318s`, mean `8.34542s`
- Extra 8 alternating:
  - `32` median `8.33790s`, mean `8.35099s`
  - `64` median `8.35612s`, mean `8.35930s`

Decision:
- Update V10 default `AC_PAR_MIN` fallback from `0` to `32`.
- Rationale:
  - `32` showed the stronger direct win against the actual retained default `0`
  - `32` kept the cleaner median story in the head-to-head with `64`
  - `64` remained plausible, but not strong enough to displace `32`

### Net

- Retained one new default change from this continuation:
  - `AC_PAR_MIN: 0 -> 32`
- Current retained defaults are now:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=32`  (updated)
  - `B_CHUNKS=4`
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=16`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## 2026-03-13 - post-`AC_PAR_MIN` retune continuation

Starting point:
- Continued from the retained `AC_PAR_MIN=32` baseline.
- AC was still the longest phase in timing, but the first recheck was `AC_SEG`
  because the AC threshold change could have shifted its old optimum.

### `AC_SEG` recheck (no retained change)

Single-run sweep on the `AC_PAR_MIN=32` baseline:
- `170000`: `8.27986s`
- `160000`: `8.35089s`
- `180000`: `8.36476s`
- `150000`: `8.37660s`
- `190000`: `8.37423s`
- `200000`: `8.42730s`

Decision:
- Keep `AC_SEG=170000`.

### Retained high-end retune: `B_CHUNKS` 4 -> 6

Why reopen it:
- The AC threshold retune changes the AC/B contention balance.
- That made the high-end `B_CHUNKS` setting worth rechecking before touching any
  new source path.

Single-run sweep on the `AC_PAR_MIN=32` baseline:
- `4`: `8.35611s`
- `2`: `8.40007s`
- `6`: `8.31756s`
- `8`: `8.34181s`
- `1`: `8.35109s`
- `3`: `8.42139s`
- `5`: `8.37249s`

12 alternating (`6` vs default `4`), both orders:
- candidate median `8.34913s`, mean `8.36963s`
- baseline median `8.36519s`, mean `8.37505s`
- delta `-0.192%` median, `-0.065%` mean

Cross-scale spot checks:
- `1e18`:
  - `4`: `2.21913s`
  - `6`: `2.21923s`
  - effectively neutral
- `1e17`:
  - `4`: `0.59136s`
  - `6`: `0.58953s`
  - slight edge for `6`

Decision:
- Update the top two runtime tiers to `b_chunks=6`:
  - `x >= 1e18`
  - `x >= 1e17`

### Net

- Retained one new runtime-tuning change from this continuation:
  - `B_CHUNKS: 4 -> 6` for the top two scale tiers
- Current retained high-end defaults are now:
  - `AC_SEG=170000`
  - `AC_PAR_MIN=32`
  - `B_CHUNKS=6`  (updated for `x >= 1e17`)
  - `D_SEG_CAP=20`
  - `D_SEG_MIN_CAP=14`
  - `D_CHUNKS=24`
  - `VM_STRIDE=16`
  - `VM_LOOKAHEAD=1`
  - `D_ADAPT_CHUNKS=0`
  - `D_AUTO_CHUNK_SELECT=0`
  - `POOL_MULT=3`

## 2026-03-13 - high-end tier retune follow-up

Starting point:
- Continued from the high-end baseline with:
  - `AC_PAR_MIN=32`
  - top-end `B_CHUNKS=6`
- This pass focused on the explicit runtime table entries for `1e17` and `1e18`
  rather than global defaults.

### Retained `1e17` retune: `AC_SEG` 190000 -> 180000

Single-run sweep at `1e17` (`AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`):
- `170000`: `0.59956s`
- `180000`: `0.58226s`
- `190000`: `0.58887s`
- `200000`: `0.58537s`
- `210000`: `0.58693s`
- `220000`: `0.59182s`

20 alternating at `1e17` (`180000` vs current `190000`), both orders:
- candidate median `0.59028s`, mean `0.59307s`
- baseline median `0.59196s`, mean `0.59418s`
- delta `-0.284%` median, `-0.187%` mean

Decision:
- Update the `1e17` runtime tier from `ac_seg=190000` to `ac_seg=180000`.

### `1e18` `AC_SEG` recheck (no retained change)

Single-run sweep at `1e18` (`AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`):
- `150000`: `2.21505s`
- `160000`: `2.24179s`
- `170000`: `2.25706s`
- `180000`: `2.22683s`
- `190000`: `2.22640s`

20 alternating at `1e18` (`150000` vs current `170000`), both orders:
- candidate median `2.23245s`, mean `2.23144s`
- baseline median `2.22201s`, mean `2.22482s`
- delta `+0.470%` median, `+0.298%` mean

Decision:
- Keep the `1e18` runtime tier at `ac_seg=170000`.

### `1e17` `D_CHUNKS` recheck on the new `AC_SEG=180000` candidate (no retained change)

Single-run sweep at `1e17`:
- `16`: `0.58306s`
- `20`: `0.58909s`
- `24`: `0.58895s`
- `28`: `0.62053s`
- `32`: `0.58789s`

20 alternating at `1e17` (`16` vs current `24`), both orders:
- candidate median `0.59134s`, mean `0.59560s`
- baseline median `0.59057s`, mean `0.59096s`
- delta `+0.130%` median, `+0.785%` mean

Decision:
- Keep `d_chunks=24` for the `1e17` tier.

### Net

- Retained one new runtime-table change from this continuation:
  - `1e17 AC_SEG: 190000 -> 180000`
- Current retained high-end runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`  (updated), `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `B_CHUNKS=6`, `D_CHUNKS=20`

## 2026-03-14 - tier split follow-up

Starting point:
- Continued from the runtime table after the `1e17 AC_SEG` retune.
- This pass checked two ideas:
  1. whether `AC_PAR_MIN` should become scale-aware
  2. whether the `1e15..1e16` row wanted a different `B_CHUNKS` value than the
     higher-end tiers

### Scale-aware `AC_PAR_MIN` recheck (no retained change)

Single-run sweep at `1e17` (`AC_SEG=180000`, `B_CHUNKS=6`, `D_CHUNKS=24`):
- `0`: `0.58646s`
- `16`: `0.59386s`
- `32`: `0.59438s`
- `48`: `0.58736s`
- `64`: `0.58836s`
- `96`: `0.58816s`

Single-run sweep at `1e18` (`AC_SEG=170000`, `B_CHUNKS=6`, `D_CHUNKS=24`):
- `0`: `2.25328s`
- `16`: `2.20233s`
- `32`: `2.20096s`
- `48`: `2.23108s`
- `64`: `2.22184s`
- `96`: `2.22231s`

20 alternating at `1e17` (`0` vs current `32`), both orders:
- candidate median `0.59231s`, mean `0.59658s`
- baseline median `0.59158s`, mean `0.59533s`
- delta `+0.123%` median, `+0.210%` mean

Decision:
- Keep global `AC_PAR_MIN=32`.

### `1e17` `B_CHUNKS` recheck on the new `AC_SEG=180000` tier (no retained change)

Single-run sweep:
- `4`: `0.58532s`
- `6`: `0.58841s`
- `8`: `0.60555s`
- `10`: `0.58405s`
- `12`: `0.58853s`

20 alternating (`10` vs current `6`), both orders:
- candidate median `0.58781s`, mean `0.59559s`
- baseline median `0.58881s`, mean `0.58989s`
- rejected due unstable long-tail regressions in the candidate mean

20 alternating (`4` vs current `6`), both orders:
- candidate median `0.59691s`, mean `0.60148s`
- baseline median `0.59028s`, mean `0.59332s`

Decision:
- Keep `B_CHUNKS=6` for the `1e17` tier.

### Retained `1e15..1e16` row retune: `B_CHUNKS` 6 -> 4

Exploration at `1e16` (`AC_SEG=200000`, `AC_PAR_MIN=32`, current `D_CHUNKS=20`):

`AC_SEG` single-run sweep:
- `160000`: `0.16615s`
- `180000`: `0.16443s`
- `200000`: `0.16209s`
- `220000`: `0.15912s`
- `240000`: `0.16487s`

`D_CHUNKS` recheck on the `AC_SEG=220000` candidate:
- `12`: `0.15823s`
- `16`: `0.16452s`
- `20`: `0.16902s`
- `24`: `0.16089s`
- `28`: `0.18732s`

`B_CHUNKS` recheck on the same aggressive candidate:
- `4`: `0.15895s`
- `6`: `0.16626s`
- `8`: `0.16734s`
- `10`: `0.16338s`
- `12`: `0.15570s`

20 alternating full-row package (`220000/12/12` vs current `200000/6/20`):
- candidate median `0.16518s`, mean `0.16350s`
- baseline median `0.16073s`, mean `0.16473s`
- rejected on median

20 alternating `AC_SEG=220000` alone vs current `200000`:
- candidate median `0.16242s`, mean `0.16225s`
- baseline median `0.15814s`, mean `0.15888s`

Read:
- The row did not want a broad package move; the aggressive candidate only
  looked good in singles.

Current-row rechecks at `1e16` (`AC_SEG=200000`, `D_CHUNKS=20`):

`D_CHUNKS` sweep:
- `12`: `0.16667s`
- `16`: `0.16307s`
- `20`: `0.16234s`
- `24`: `0.16429s`
- `28`: `0.17388s`

`B_CHUNKS` sweep:
- `4`: `0.15919s`
- `6`: `0.16651s`
- `8`: `0.16343s`
- `10`: `0.16830s`
- `12`: `0.16264s`

40 alternating at `1e16` (`4` vs current `6`), both orders:
- candidate median `0.15892s`, mean `0.16041s`
- baseline median `0.16134s`, mean `0.16141s`
- delta `-1.500%` median, `-0.620%` mean

20 alternating spot check at `1e15` (`4` vs current `6`):
- candidate median `0.04666s`, mean `0.04652s`
- baseline median `0.04679s`, mean `0.04724s`

Decision:
- Update the `x >= 1e15` runtime row from `b_chunks=6` to `b_chunks=4`.

### Net

- Retained one new runtime-table change from this continuation:
  - `1e15..1e16 B_CHUNKS: 6 -> 4`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `B_CHUNKS=4`  (updated), `D_CHUNKS=20`

## 2026-03-14 - low-tier runtime retune

Starting point:
- Continued from the runtime table after the `1e15..1e16 B_CHUNKS` split.
- First rechecked the updated `1e15..1e16` row to see whether that new balance
  had shifted `AC_PAR_MIN`, `AC_SEG`, or `D_CHUNKS`.

### `1e15..1e16` row recheck (no retained change)

At `1e16` on the current row (`AC_SEG=200000`, `B_CHUNKS=4`, `D_CHUNKS=20`):

`AC_PAR_MIN` single-run sweep:
- `0`: `0.16332s`
- `16`: `0.28508s`
- `32`: `0.29408s`
- `48`: `0.29922s`
- `64`: `0.15339s`
- `96`: `0.29937s`

20 alternating (`64` vs current `32`):
- candidate median `0.16171s`, mean `0.16173s`
- baseline median `0.15962s`, mean `0.16128s`

20 alternating (`0` vs current `32`):
- candidate median `0.15983s`, mean `0.16079s`
- baseline median `0.15937s`, mean `0.15960s`

Decision:
- Keep global `AC_PAR_MIN=32`.

`AC_SEG` single-run sweep:
- `160000`: `0.22960s`
- `180000`: `0.28483s`
- `200000`: `0.29249s`
- `220000`: `0.15824s`
- `240000`: `0.29964s`

20 alternating (`220000` vs current `200000`):
- candidate median `0.16215s`, mean `0.16216s`
- baseline median `0.16059s`, mean `0.16050s`

Decision:
- Keep `AC_SEG=200000` for the `1e15..1e16` row.

`D_CHUNKS` single-run sweep:
- `12`: `0.29626s`
- `16`: `0.16454s`
- `20`: `0.15978s`
- `24`: `0.16117s`
- `28`: `0.18281s`

Decision:
- Keep `D_CHUNKS=20`.

### Retained low-tier split: `x < 1e15` `B_CHUNKS` 8 -> 4

Motivation:
- The lower row still carried the older `B_CHUNKS=8` fallback.
- A quick `1e14` sweep showed an obvious gap between `4` and `8`, making this
  the most credible remaining runtime-table move.

At `1e14` (`AC_SEG=200000`, `AC_PAR_MIN=32`, `D_CHUNKS=24`):

Single-run sweeps:

`B_CHUNKS`:
- `2`: `0.01878s`
- `4`: `0.01735s`
- `6`: `0.01765s`
- `8`: `0.02023s`
- `10`: `0.02066s`
- `12`: `0.02228s`

`AC_SEG`:
- `120000`: `0.01838s`
- `160000`: `0.01932s`
- `200000`: `0.01881s`
- `240000`: `0.01813s`
- `280000`: `0.01791s`

`D_CHUNKS`:
- `12`: `0.01935s`
- `16`: `0.01995s`
- `20`: `0.01917s`
- `24`: `0.01722s`
- `28`: `0.01903s`
- `32`: `0.01829s`

40 alternating at `1e14` (`4` vs current `8`):
- candidate median `0.01745s`, mean `0.01740s`
- baseline median `0.01927s`, mean `0.01924s`
- delta `-9.44%` median, `-9.56%` mean

40 alternating at `1e13` (`4` vs current `8`):
- candidate median `0.00956s`, mean `0.00962s`
- baseline median `0.01073s`, mean `0.01076s`
- delta `-10.90%` median, `-10.59%` mean

40 alternating at `1e12` (`4` vs current `8`):
- candidate median `0.00627s`, mean `0.00636s`
- baseline median `0.00639s`, mean `0.00642s`
- delta `-1.88%` median, `-0.93%` mean

Decision:
- Update the `x < 1e15` runtime row from `b_chunks=8` to `b_chunks=4`.

### Net

- Retained one new runtime-table change from this continuation:
  - `x < 1e15 B_CHUNKS: 8 -> 4`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x < 1e15`: `AC_SEG=200000`, `B_CHUNKS=4`  (updated), `D_CHUNKS=24`

## 2026-03-14 - low-tier AC threshold split

Starting point:
- Continued from the low-tier `B_CHUNKS=4` row.
- The next question was whether `AC_PAR_MIN` should also split by scale instead
  of staying at the same global `32` everywhere.

### `1e15..1e16` row recheck (no retained change)

At `1e16` on the current row (`AC_SEG=200000`, `B_CHUNKS=4`, `D_CHUNKS=20`):

20 alternating (`64` vs current `32`):
- candidate median `0.16171s`, mean `0.16173s`
- baseline median `0.15962s`, mean `0.16128s`

20 alternating (`220000` vs current `200000`):
- candidate median `0.16215s`, mean `0.16216s`
- baseline median `0.16059s`, mean `0.16050s`

20 alternating (`0` vs current `32`):
- candidate median `0.15983s`, mean `0.16079s`
- baseline median `0.15937s`, mean `0.15960s`

Decision:
- Keep the `1e15..1e16` row unchanged.

### Retained low-tier split: `x < 1e15` `AC_PAR_MIN` 32 -> 64

Low-tier single-run sweeps on the retained low row (`AC_SEG=200000`,
`B_CHUNKS=4`, `D_CHUNKS=24`):

At `1e14`:
- `0`: `0.02133s`
- `8`: `0.02266s`
- `16`: `0.02106s`
- `32`: `0.02244s`
- `48`: `0.01788s`
- `64`: `0.01723s`

At `1e13`:
- `0`: `0.02003s`
- `8`: `0.01533s`
- `16`: `0.02122s`
- `32`: `0.00985s`
- `48`: `0.00965s`
- `64`: `0.00950s`

40 alternating at `1e14` (`64` vs current `32`):
- candidate median `0.01713s`, mean `0.01777s`
- baseline median `0.01715s`, mean `0.01778s`

40 alternating at `1e13` (`64` vs current `32`):
- candidate median `0.00965s`, mean `0.01194s`
- baseline median `0.00975s`, mean `0.01215s`

40 alternating at `1e12` (`64` vs current `32`):
- candidate median `0.00626s`, mean `0.00628s`
- baseline median `0.00628s`, mean `0.00632s`

Decision:
- Add `ac_par_min` to `RuntimeTuning`.
- Keep `AC_PAR_MIN=32` for `x >= 1e15`.
- Set the `x < 1e15` row to `ac_par_min=64`.

### Net

- Retained one new runtime-table change from this continuation:
  - `x < 1e15 AC_PAR_MIN: 32 -> 64`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=32`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x < 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=64`  (updated), `B_CHUNKS=4`, `D_CHUNKS=24`

## 2026-03-14 - low-tier D split

Starting point:
- Continued from the low-tier row after the `AC_PAR_MIN=64` split.
- Goal: see whether that new low-row balance had shifted `AC_SEG` or `D_CHUNKS`
  enough to justify a deeper split below `1e14`.

### `1e14` low-row recheck (no retained change)

Current low row at `1e14`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=4`,
`D_CHUNKS=24`.

Single-run sweeps:

`AC_SEG`:
- `120000`: `0.03685s`
- `160000`: `0.02255s`
- `200000`: `0.01682s`
- `240000`: `0.01737s`
- `280000`: `0.01667s`
- `320000`: `0.02649s`

`D_CHUNKS`:
- `12`: `0.04219s`
- `16`: `0.01628s`
- `20`: `0.01731s`
- `24`: `0.01693s`
- `28`: `0.01632s`
- `32`: `0.01680s`

`B_CHUNKS`:
- `2`: `0.03468s`
- `4`: `0.02897s`
- `6`: `0.03065s`
- `8`: `0.01996s`

Longer checks:
- `AC_SEG=280000` vs current `200000`:
  - candidate median `0.01693s`, mean `0.01748s`
  - baseline median `0.01683s`, mean `0.01754s`
- `D_CHUNKS=16` vs current `24`:
  - candidate median `0.01690s`, mean `0.01752s`
  - baseline median `0.01686s`, mean `0.01722s`

Decision:
- Keep the `1e14` row unchanged.

### `1e13` recheck

Single-run sweeps at `1e13` on the same low row:

`AC_SEG`:
- `120000`: `0.00944s`
- `160000`: `0.00980s`
- `200000`: `0.00927s`
- `240000`: `0.01119s`
- `280000`: `0.00933s`
- `320000`: `0.00938s`

`D_CHUNKS`:
- `12`: `0.00935s`
- `16`: `0.01038s`
- `20`: `0.01056s`
- `24`: `0.00944s`
- `28`: `0.00953s`
- `32`: `0.00959s`

`B_CHUNKS`:
- `2`: `0.01362s`
- `4`: `0.01355s`
- `6`: `0.01505s`
- `8`: `0.01093s`

Longer checks:
- `B_CHUNKS=8` vs current `4`:
  - candidate median `0.01071s`, mean `0.01076s`
  - baseline median `0.00965s`, mean `0.00968s`
  - rejected immediately as another single-run false positive
- `D_CHUNKS=12` vs current `24`:
  - candidate median `0.00963s`, mean `0.00969s`
  - baseline median `0.00966s`, mean `0.00972s`

### Retained sub-tier split: `1e12..1e13` `D_CHUNKS` 24 -> 12

Why split instead of changing the whole low tier:
- `1e14` did not want smaller `D_CHUNKS`.
- `1e11` also failed to support it.
- The evidence pointed specifically at the middle of the low tier.

Supporting checks:
- `1e12` (`12` vs current `24`):
  - candidate median `0.00633s`, mean `0.00635s`
  - baseline median `0.00641s`, mean `0.00640s`
- `1e11` (`12` vs current `24`):
  - candidate median `0.00547s`, mean `0.00557s`
  - baseline median `0.00536s`, mean `0.00550s`
  - rejected for the lower decade

Decision:
- Add a new runtime tier:
  - `x >= 1e12`: `D_CHUNKS=12`
- Keep the lower row (`x < 1e12`) at `D_CHUNKS=24`.

### Net

- Retained one new runtime-table change from this continuation:
  - `1e12..1e13 D_CHUNKS: 24 -> 12`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=32`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x >= 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=4`, `D_CHUNKS=12`  (updated)
  - `x < 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=4`, `D_CHUNKS=24`

## Continuation Pass: Bottom-Row B Re-Tune Below 1e12

- Continued from the new `x >= 1e12` split and re-opened the remaining bottom row
  at `1e11`.
- Goal: determine whether `x < 1e12` wanted a different `AC_SEG`, `B_CHUNKS`, or
  `D_CHUNKS` balance now that `1e12..1e13` had moved off onto `D_CHUNKS=12`.

### `1e11` exploratory sweep on the retained bottom row

Current row before this pass: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=4`,
`D_CHUNKS=24`.

`AC_PAR_MIN`:
- `0`: `0.00612s`
- `16`: `0.00605s`
- `32`: `0.00532s`
- `64`: `0.00523s`
- `96`: `0.00557s`
- `128`: `0.00545s`

`AC_SEG`:
- `120000`: `0.00548s`
- `160000`: `0.00498s`
- `200000`: `0.00560s`
- `240000`: `0.00562s`
- `280000`: `0.00531s`
- `320000`: `0.00528s`

`B_CHUNKS`:
- `2`: `0.00502s`
- `4`: `0.00522s`
- `6`: `0.00517s`
- `8`: `0.00665s`

`D_CHUNKS`:
- `8`: `0.00534s`
- `12`: `0.00546s`
- `16`: `0.00533s`
- `24`: `0.00564s`
- `32`: `0.00570s`

### Bidirectional validation at `1e11`

100 runs per side total (50 candidate-first + 50 baseline-first):

- `AC_SEG=160000` vs current `200000`:
  - candidate median `0.00541s`, mean `0.00552s`
  - baseline median `0.00541s`, mean `0.00547s`
  - rejected; single-run signal disappeared
- `B_CHUNKS=2` vs current `4`:
  - candidate median `0.00534s`, mean `0.00542s`
  - baseline median `0.00537s`, mean `0.00549s`
- `D_CHUNKS=8` vs current `24`:
  - candidate median `0.00544s`, mean `0.00555s`
  - baseline median `0.00547s`, mean `0.00557s`
  - plausible, but weaker than the B retune

### Interaction check

- The combined candidate (`B_CHUNKS=2`, `D_CHUNKS=8`) beat the old bottom row:
  - `1e11`: candidate median `0.00532s`, mean `0.00539s`
  - `1e11`: baseline median `0.00541s`, mean `0.00551s`
  - `1e10`: candidate median `0.00477s`, mean `0.00487s`
  - `1e10`: baseline median `0.00494s`, mean `0.00501s`
- But direct A/B showed that package was not the true optimum:
  - `1e11 combo` vs `B_CHUNKS=2` only:
    - candidate median `0.00541s`, mean `0.00550s`
    - baseline median `0.00530s`, mean `0.00543s`
  - `1e11 combo` vs `D_CHUNKS=8` only:
    - candidate median `0.00539s`, mean `0.00547s`
    - baseline median `0.00530s`, mean `0.00545s`
- So the combined row was rejected as another low-end false positive.

### Direct B vs D choice

- `1e11`, `B_CHUNKS=2` vs `D_CHUNKS=8`:
  - candidate median `0.00523s`, mean `0.00537s`
  - baseline median `0.00536s`, mean `0.00548s`
- `1e10`, `B_CHUNKS=2` vs current `4`:
  - candidate median `0.00483s`, mean `0.00493s`
  - baseline median `0.00485s`, mean `0.00494s`
- `1e10`, `B_CHUNKS=2` vs `D_CHUNKS=8`:
  - candidate median `0.00487s`, mean `0.00490s`
  - baseline median `0.00491s`, mean `0.00499s`

### Retained bottom-row retune: `x < 1e12` `B_CHUNKS` 4 -> 2

Decision:
- Keep the remaining bottom row on:
  - `AC_SEG=200000`
  - `AC_PAR_MIN=64`
  - `B_CHUNKS=2`  (updated)
  - `D_CHUNKS=24`

### Net

- Retained one new low-end runtime-table change from this continuation:
  - `x < 1e12 B_CHUNKS: 4 -> 2`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=32`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x >= 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=4`, `D_CHUNKS=12`
  - `x < 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=2`  (updated), `D_CHUNKS=24`

## Continuation Pass: `1e12..1e13` B Recheck After Low-End Split

- Continued from the newly retained `x < 1e12 B_CHUNKS=2` row.
- Goal: check whether the neighboring `x >= 1e12` row had shifted now that the
  lower decade was no longer sharing the same B-side default.

### Exploratory sweeps on the current `x >= 1e12` row

Current row before this pass:
- `AC_SEG=200000`
- `AC_PAR_MIN=64`
- `B_CHUNKS=4`
- `D_CHUNKS=12`

At `1e13` (`10 Trillion`):

`AC_SEG`:
- `120000`: `0.00960s`
- `160000`: `0.00958s`
- `200000`: `0.01028s`
- `240000`: `0.00960s`
- `280000`: `0.00920s`
- `320000`: `0.00917s`

`AC_PAR_MIN`:
- `0`: `0.00984s`
- `16`: `0.00904s`
- `32`: `0.00982s`
- `48`: `0.00921s`
- `64`: `0.00955s`
- `96`: `0.00894s`
- `128`: `0.00977s`

`B_CHUNKS`:
- `2`: `0.00929s`
- `4`: `0.00953s`
- `6`: `0.00974s`
- `8`: `0.01119s`

`D_CHUNKS`:
- `8`: `0.00947s`
- `12`: `0.00952s`
- `16`: `0.01026s`
- `20`: `0.00959s`
- `24`: `0.01012s`

At `1e12` (`1 Trillion`):

`AC_SEG`:
- `120000`: `0.00686s`
- `160000`: `0.00645s`
- `200000`: `0.00640s`
- `240000`: `0.00595s`
- `280000`: `0.00621s`
- `320000`: `0.00642s`

`AC_PAR_MIN`:
- `0`: `0.00618s`
- `16`: `0.00601s`
- `32`: `0.00655s`
- `48`: `0.00629s`
- `64`: `0.00708s`
- `96`: `0.00686s`
- `128`: `0.00632s`

`B_CHUNKS`:
- `2`: `0.00622s`
- `4`: `0.00635s`
- `6`: `0.00613s`
- `8`: `0.00604s`

`D_CHUNKS`:
- `8`: `0.00618s`
- `12`: `0.00648s`
- `16`: `0.00642s`
- `20`: `0.00619s`
- `24`: `0.00675s`

### Bidirectional validation

60 runs per side total (30 candidate-first + 30 baseline-first):

At `1e13`:
- `B_CHUNKS=2` vs current `4`:
  - candidate median `0.00932s`, mean `0.00936s`
  - baseline median `0.00968s`, mean `0.00972s`
  - clear keep signal
- `AC_SEG=280000` vs current `200000`:
  - candidate median `0.00959s`, mean `0.00961s`
  - baseline median `0.00959s`, mean `0.00961s`
  - no move
- `AC_SEG=320000` vs current `200000`:
  - candidate median `0.00971s`, mean `0.00976s`
  - baseline median `0.00960s`, mean `0.00964s`
  - rejected
- `AC_PAR_MIN=96` vs current `64`:
  - candidate median `0.00972s`, mean `0.00971s`
  - baseline median `0.00953s`, mean `0.00958s`
  - rejected
- `AC_PAR_MIN=16` vs current `64`:
  - candidate median `0.00965s`, mean `0.00969s`
  - baseline median `0.00961s`, mean `0.00965s`
  - rejected

At `1e12`:
- `B_CHUNKS=8` vs current `4`:
  - candidate median `0.00643s`, mean `0.00644s`
  - baseline median `0.00625s`, mean `0.00631s`
  - rejected despite strong single-run bait
- `B_CHUNKS=6` vs current `4`:
  - candidate median `0.00632s`, mean `0.00639s`
  - baseline median `0.00630s`, mean `0.00635s`
  - no move
- `AC_SEG=240000` vs current `200000`:
  - candidate median `0.00633s`, mean `0.00635s`
  - baseline median `0.00636s`, mean `0.00636s`
  - too small to keep
- `AC_PAR_MIN=16` vs current `64`:
  - candidate median `0.00631s`, mean `0.00636s`
  - baseline median `0.00631s`, mean `0.00638s`
  - effectively flat
- `D_CHUNKS=8` vs current `12`:
  - candidate median `0.00628s`, mean `0.00631s`
  - baseline median `0.00626s`, mean `0.00632s`
  - split/noisy, not enough to keep

### Final B cross-check for the row

100 runs per side total (50 candidate-first + 50 baseline-first):

- `1e13`, `B_CHUNKS=2` vs current `4`:
  - candidate median `0.00929s`, mean `0.00934s`
  - baseline median `0.00972s`, mean `0.00975s`
- `1e12`, `B_CHUNKS=2` vs current `4`:
  - candidate median `0.00628s`, mean `0.00630s`
  - baseline median `0.00634s`, mean `0.00638s`
- `1e12`, `B_CHUNKS=2` vs `6`:
  - candidate median `0.00636s`, mean `0.00640s`
  - baseline median `0.00637s`, mean `0.00642s`

### Retained row retune: `x >= 1e12` `B_CHUNKS` 4 -> 2

Decision:
- Update the `x >= 1e12` row from:
  - `B_CHUNKS=4`
  to:
  - `B_CHUNKS=2`

Rationale:
- The B-side win at `1e13` is large and stable.
- The same direction also holds at `1e12`.
- None of the accompanying AC or D candidates showed a robust enough gain to
  justify a broader row rewrite.

### Net

- Retained one new runtime-table change from this continuation:
  - `x >= 1e12 B_CHUNKS: 4 -> 2`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=32`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x >= 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=2`  (updated), `D_CHUNKS=12`
  - `x < 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=2`, `D_CHUNKS=24`

## Continuation Pass: `1e14` Coverage Audit on the New `x >= 1e12` Row

- After landing `x >= 1e12 B_CHUNKS=2`, I rechecked the actual row coverage and
  confirmed that it also affects `1e14`, not just `1e12..1e13`.
- That made an immediate `1e14` audit mandatory before treating the new row as
  settled.

### First safety check: does the new row regress `1e14`?

Current row under audit:
- `AC_SEG=200000`
- `AC_PAR_MIN=64`
- `B_CHUNKS=2`
- `D_CHUNKS=12`

80 runs per side total (40 candidate-first + 40 baseline-first):

- current row vs `B_CHUNKS=4`, `D_CHUNKS=12`:
  - candidate median `0.01686s`, mean `0.01697s`
  - baseline median `0.01711s`, mean `0.01716s`
- current row vs `B_CHUNKS=2`, `D_CHUNKS=24`:
  - candidate median `0.01680s`, mean `0.01700s`
  - baseline median `0.01707s`, mean `0.01722s`
- current row vs `B_CHUNKS=4`, `D_CHUNKS=24`:
  - candidate median `0.01707s`, mean `0.01716s`
  - baseline median `0.01720s`, mean `0.01734s`

Read:
- The new row did not regress `1e14`; it actually held up against all three
  nearby older combinations.

### Re-opened `1e14` search on the new row (no retained change)

Single-run sweeps on the new `1e14` baseline:

`AC_SEG`:
- `120000`: `0.01766s`
- `160000`: `0.01746s`
- `200000`: `0.01700s`
- `240000`: `0.01651s`
- `280000`: `0.01636s`
- `320000`: `0.01697s`

`AC_PAR_MIN`:
- `0`: `0.01647s`
- `16`: `0.01735s`
- `32`: `0.01635s`
- `48`: `0.01651s`
- `64`: `0.01709s`
- `96`: `0.01603s`
- `128`: `0.01767s`

`B_CHUNKS`:
- `2`: `0.01808s`
- `4`: `0.01734s`
- `6`: `0.01753s`
- `8`: `0.01999s`

`D_CHUNKS`:
- `8`: `0.01788s`
- `12`: `0.01840s`
- `16`: `0.01725s`
- `20`: `0.01819s`
- `24`: `0.01743s`

80 runs per side total (40 candidate-first + 40 baseline-first):

- `AC_SEG=280000` vs current `200000`:
  - candidate median `0.01698s`, mean `0.01713s`
  - baseline median `0.01691s`, mean `0.01705s`
  - rejected
- `AC_PAR_MIN=96` vs current `64`:
  - candidate median `0.01685s`, mean `0.01701s`
  - baseline median `0.01699s`, mean `0.01710s`
  - plausible but not strong enough yet
- `AC_PAR_MIN=32` vs current `64`:
  - candidate median `0.01691s`, mean `0.01703s`
  - baseline median `0.01695s`, mean `0.01705s`
  - too small to keep
- `D_CHUNKS=16` vs current `12`:
  - candidate median `0.01726s`, mean `0.01731s`
  - baseline median `0.01679s`, mean `0.01697s`
  - rejected clearly

Focused `AC_PAR_MIN` retest at `1e14`:

Single-run sweep:
- `32`: `0.01632s`
- `48`: `0.01777s`
- `64`: `0.01652s`
- `80`: `0.01762s`
- `96`: `0.01654s`
- `112`: `0.01632s`
- `128`: `0.01683s`

100 runs per side total (50 candidate-first + 50 baseline-first):

- `80` vs current `64`:
  - candidate median `0.01688s`, mean `0.01704s`
  - baseline median `0.01691s`, mean `0.01711s`
- `96` vs current `64`:
  - candidate median `0.01684s`, mean `0.01707s`
  - baseline median `0.01686s`, mean `0.01697s`
- `112` vs current `64`:
  - candidate median `0.01686s`, mean `0.01700s`
  - baseline median `0.01696s`, mean `0.01703s`
- `96` vs `80`:
  - candidate median `0.01706s`, mean `0.01709s`
  - baseline median `0.01672s`, mean `0.01697s`
- `96` vs `32`:
  - candidate median `0.01698s`, mean `0.01718s`
  - baseline median `0.01690s`, mean `0.01703s`

Decision:
- No retained `1e14` follow-up move from this continuation.
- The important outcome was the coverage audit itself:
  - the new `x >= 1e12` row is safe at `1e14`
  - none of the tempting singles justified another split yet

## Continuation Pass: Re-opened `x >= 1e15` Row and High-End Follow-Up

- After the low/mid-low rows were cleaned up, the next search reopened the still
  broad `x >= 1e15` row and then moved upward again into the `1e17`/`1e18`
  tiers.

### `1e15..1e16` split search (no retained change)

Current shared row at the start of this pass:
- `AC_SEG=200000`
- `AC_PAR_MIN=32`
- `B_CHUNKS=4`
- `D_CHUNKS=20`

Single-run sweeps at `1e16` (`10 Quadrillion`):

`AC_SEG`:
- `160000`: `0.15690s`
- `180000`: `0.16013s`
- `200000`: `0.16606s`
- `220000`: `0.15922s`
- `240000`: `0.15216s`
- `260000`: `0.16207s`

`AC_PAR_MIN`:
- `0`: `0.15618s`
- `16`: `0.15836s`
- `32`: `0.15986s`
- `48`: `0.16206s`
- `64`: `0.16170s`
- `96`: `0.16035s`
- `128`: `0.15426s`

`B_CHUNKS`:
- `2`: `0.16462s`
- `4`: `0.16785s`
- `6`: `0.16499s`
- `8`: `0.16572s`
- `10`: `0.16419s`

`D_CHUNKS`:
- `12`: `0.15945s`
- `16`: `0.15750s`
- `20`: `0.15740s`
- `24`: `0.16461s`
- `28`: `0.16267s`

Single-run sweeps at `1e15` (`1 Quadrillion`):

`AC_SEG`:
- `160000`: `0.05002s`
- `180000`: `0.04767s`
- `200000`: `0.04480s`
- `220000`: `0.04607s`
- `240000`: `0.04538s`
- `260000`: `0.04775s`

`AC_PAR_MIN`:
- `0`: `0.04723s`
- `16`: `0.05110s`
- `32`: `0.05005s`
- `48`: `0.04601s`
- `64`: `0.04913s`
- `96`: `0.04704s`
- `128`: `0.04827s`

`B_CHUNKS`:
- `2`: `0.04967s`
- `4`: `0.04675s`
- `6`: `0.04562s`
- `8`: `0.05086s`
- `10`: `0.04593s`

`D_CHUNKS`:
- `12`: `0.04757s`
- `16`: `0.04705s`
- `20`: `0.04973s`
- `24`: `0.04664s`
- `28`: `0.04791s`

Bidirectional checks:

At `1e16`:
- `AC_SEG=240000` vs current `200000`:
  - short window: candidate median `0.18369s`, mean `0.18027s`
  - short window: baseline median `0.18956s`, mean `0.18316s`
  - looked worth pursuing
- `AC_SEG=160000` vs current `200000`:
  - candidate median `0.18237s`, mean `0.18313s`
  - baseline median `0.17646s`, mean `0.17906s`
  - rejected
- `AC_PAR_MIN=0` vs current `32`:
  - candidate median `0.18600s`, mean `0.18120s`
  - baseline median `0.18302s`, mean `0.18274s`
  - rejected
- `AC_PAR_MIN=128` vs current `32`:
  - candidate median `0.18272s`, mean `0.18570s`
  - baseline median `0.18419s`, mean `0.18472s`
  - mixed, not enough
- `B_CHUNKS=2` vs current `4`:
  - candidate median `0.16474s`, mean `0.17288s`
  - baseline median `0.16816s`, mean `0.17402s`
  - positive but smaller than the AC signal
- `B_CHUNKS=10` vs current `4`:
  - candidate median `0.16074s`, mean `0.16162s`
  - baseline median `0.16226s`, mean `0.16190s`
- `D_CHUNKS=16` vs current `20`:
  - candidate median `0.16303s`, mean `0.16172s`
  - baseline median `0.16099s`, mean `0.16064s`
  - rejected

At `1e15`:
- `AC_PAR_MIN=48` vs current `32`:
  - short window: candidate median `0.04835s`, mean `0.06553s`
  - short window: baseline median `0.04915s`, mean `0.07540s`
  - looked promising, but had obvious tail noise
- `B_CHUNKS=6` vs current `4`:
  - candidate median `0.04867s`, mean `0.06717s`
  - baseline median `0.04855s`, mean `0.07078s`
- `B_CHUNKS=10` vs current `4`:
  - candidate median `0.04900s`, mean `0.06475s`
  - baseline median `0.04994s`, mean `0.06603s`
- `D_CHUNKS=24` vs current `20`:
  - candidate median `0.04925s`, mean `0.07265s`
  - baseline median `0.04952s`, mean `0.07193s`

Follow-up combo screens:
- `1e16 AC240 + B6` vs base:
  - candidate median `0.16326s`, mean `0.16234s`
  - baseline median `0.16048s`, mean `0.16127s`
  - rejected
- `1e16 AC240 + B8` vs base:
  - candidate median `0.16170s`, mean `0.16208s`
  - baseline median `0.15991s`, mean `0.16071s`
  - rejected
- `1e16 AC240 + AC_PAR_MIN=96` vs `AC240`:
  - candidate median `0.15947s`, mean `0.15919s`
  - baseline median `0.16164s`, mean `0.16293s`
  - looked better than `AC240` alone
- `1e15 AC_PAR_MIN=48 + D12` vs base:
  - candidate median `0.04663s`, mean `0.04669s`
  - baseline median `0.04734s`, mean `0.04697s`
  - looked worth checking against `AC_PAR_MIN=48` alone
- `1e15 AC_PAR_MIN=48 + B10` vs `AC_PAR_MIN=48`:
  - candidate median `0.04730s`, mean `0.04771s`
  - baseline median `0.04730s`, mean `0.04734s`
  - no gain
- `1e15 AC_PAR_MIN=48 + D12` vs `AC_PAR_MIN=48`:
  - candidate median `0.04725s`, mean `0.04707s`
  - baseline median `0.04652s`, mean `0.04654s`
  - rejected

Long confirmation that killed the split:
- `1e16 AC240` vs base:
  - candidate median `0.16059s`, mean `0.16126s`
  - baseline median `0.16131s`, mean `0.16100s`
  - median edge survived, mean did not
- `1e16 AC240 + AC_PAR_MIN=96` vs base:
  - candidate median `0.16220s`, mean `0.16255s`
  - baseline median `0.16107s`, mean `0.16111s`
  - rejected
- `1e16 AC240 + AC_PAR_MIN=96` vs `AC240`:
  - candidate median `0.16329s`, mean `0.16366s`
  - baseline median `0.16211s`, mean `0.16301s`
  - rejected
- `1e15 AC_PAR_MIN=48` vs base:
  - candidate median `0.04716s`, mean `0.04717s`
  - baseline median `0.04691s`, mean `0.04711s`
  - rejected

Decision:
- No retained `1e15`/`1e16` split from this continuation.
- Too many mid-row candidates improved on one statistic and failed on the other
  once the windows were long enough.

### High-end recheck

Single-run sweeps at `1e17` (`100 Quadrillion`, current row
`AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`):

`AC_SEG`:
- `160000`: `0.58980s`
- `170000`: `0.59331s`
- `180000`: `0.58782s`
- `190000`: `0.58911s`
- `200000`: `0.58941s`
- `220000`: `0.59329s`

`AC_PAR_MIN`:
- `0`: `0.58716s`
- `16`: `0.58256s`
- `32`: `0.59107s`
- `48`: `0.58639s`
- `64`: `0.59081s`
- `96`: `0.58901s`

`B_CHUNKS`:
- `4`: `0.59087s`
- `6`: `0.63348s`
- `8`: `0.59360s`
- `10`: `0.58787s`
- `12`: `0.59792s`

`D_CHUNKS`:
- `16`: `0.59312s`
- `20`: `0.59446s`
- `24`: `0.58852s`
- `28`: `0.59086s`
- `32`: `0.58529s`

Bidirectional validation at `1e17`:
- `AC_PAR_MIN=16` vs current `32`:
  - candidate median `0.58987s`, mean `0.59228s`
  - baseline median `0.59128s`, mean `0.59978s`
  - positive, but smaller than the B-side move
- `B_CHUNKS=10` vs current `6`:
  - first check: candidate median `0.58716s`, mean `0.59020s`
  - first check: baseline median `0.59313s`, mean `0.59642s`
- `D_CHUNKS=32` vs current `24`:
  - candidate median `0.59165s`, mean `0.59331s`
  - baseline median `0.59125s`, mean `0.59172s`
  - rejected

Long confirmation:
- `B_CHUNKS=10` vs current `6`:
  - candidate median `0.58783s`, mean `0.58902s`
  - baseline median `0.59210s`, mean `0.59671s`
  - delta `-0.723%` median, `-1.289%` mean
- `B_CHUNKS=10` + `AC_PAR_MIN=16` vs `B_CHUNKS=10`:
  - candidate median `0.58926s`, mean `0.59268s`
  - baseline median `0.58742s`, mean `0.59189s`
  - rejected

### `1e18` spot recheck (no retained change)

Single-run sweep at `1e18` (`1 Quintillion`, current row
`AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`):

`AC_SEG`:
- `150000`: `2.20595s`
- `160000`: `2.22023s`
- `170000`: `2.20893s`
- `180000`: `2.25847s`
- `190000`: `2.26459s`

`AC_PAR_MIN`:
- `0`: `2.24496s`
- `16`: `2.25494s`
- `32`: `2.24498s`
- `48`: `2.21811s`
- `64`: `2.23935s`
- `96`: `2.24626s`

`B_CHUNKS`:
- `4`: `2.23317s`
- `6`: `2.22386s`
- `8`: `2.21295s`
- `10`: `2.22677s`

`D_CHUNKS`:
- `16`: `2.23214s`
- `20`: `2.21173s`
- `24`: `2.22171s`
- `28`: `2.21778s`
- `32`: `2.21495s`

Serial A/B on the strongest threshold candidate:
- `AC_PAR_MIN=48` vs current `32`:
  - candidate median `2.21846s`, mean `2.21887s`
  - baseline median `2.21812s`, mean `2.22038s`
  - effectively flat

### Retained high-end retune: `1e17` `B_CHUNKS` 6 -> 10

Decision:
- Update the `1e17` runtime row from:
  - `B_CHUNKS=6`
  to:
  - `B_CHUNKS=10`
- Keep:
  - `1e17 AC_PAR_MIN=32`
  - `1e17 D_CHUNKS=24`
  - `1e18` row unchanged

### Net

- Retained one new runtime-table change from this continuation:
  - `1e17 B_CHUNKS: 6 -> 10`
- Current retained runtime tiers are now:
  - `x >= 1e18`: `AC_SEG=170000`, `AC_PAR_MIN=32`, `B_CHUNKS=6`, `D_CHUNKS=24`
  - `x >= 1e17`: `AC_SEG=180000`, `AC_PAR_MIN=32`, `B_CHUNKS=10`  (updated), `D_CHUNKS=24`
  - `x >= 1e15`: `AC_SEG=200000`, `AC_PAR_MIN=32`, `B_CHUNKS=4`, `D_CHUNKS=20`
  - `x >= 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=2`, `D_CHUNKS=12`
  - `x < 1e12`: `AC_SEG=200000`, `AC_PAR_MIN=64`, `B_CHUNKS=2`, `D_CHUNKS=24`
