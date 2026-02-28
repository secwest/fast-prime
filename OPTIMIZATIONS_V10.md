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
