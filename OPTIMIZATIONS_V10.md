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
