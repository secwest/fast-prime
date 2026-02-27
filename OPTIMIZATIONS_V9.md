# V9 Optimization Log

## Baseline

- Source: `src/bin/prime_count_v9.rs` (forked from V8)
- Goal: improve AC wall-time without modifying V8
- Focus area: AC segment loop overhead for wide `b` values
- Compile parity requirement (from V8 docs):
  - `cargo +nightly build --release --bin prime_count_v* -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
  - Uses `.cargo/config.toml` rustflags (`target-cpu=native`, unroll threshold, MIR opts, `-Ztune-cpu=arrowlake`)

## Opt 1: AC Wide-Index Segment Bucketing

### Hypothesis

In `compute_ac`, every segment iterates all wide `b` entries, but most of those entries are inactive for that segment and immediately return after range checks. Bucketing wide entries per segment should reduce no-op work and rayon dispatch overhead.

### Change

- Replaced global `wide_indices` scan per segment with:
  - `wide_ranges: Vec<(b_idx, seg_lo, seg_hi)>`
  - `wide_by_seg: Vec<Vec<b_idx>>`
- Each segment now processes:
  - `wide_by_seg[seg]` (active wide entries only)
  - plus existing `narrow_for_seg`

### Expected Impact

- Lower per-segment loop overhead in AC
- Potentially improved wall time at Max i64

### Status

- Implemented in V9
- Initial A/B benchmark completed

### Initial Benchmark (Max i64, LIMIT=9223372036854775807)

- Run order: V8, V9, V8, V9
- V8: `8.33240s`, `8.56437s` (median `8.44839s`)
- V9: `8.42584s`, `8.64194s` (median `8.53389s`)
- Correctness: all runs matched expected `216289611853439384`

### Result

- **Regression** in initial sample (~`+1.0%` median vs V8)
- Kept in V9 branch for further iteration and profiling

### Re-test With V8 Compile Options (nightly + build-std)

- Build commands used:
  - `cargo +nightly build --release --bin prime_count_v8 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
  - `cargo +nightly build --release --bin prime_count_v9 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
- Run order: V8, V9, V8, V9
- V8: `8.50764s`, `8.42613s` (median `8.46689s`)
- V9: `8.43184s`, `8.70700s` (median `8.56942s`)
- Correctness: all runs matched expected `216289611853439384`

### Updated Result

- V9 Opt 1 remains a **regression/noise-negative** candidate under the documented V8 compile configuration.

## Opt 2: AC Wide Active-Set Event Sweep

### Hypothesis

Opt 1 reduced no-op wide work but paid heavy setup/memory overhead by duplicating wide indices into `wide_by_seg[seg]` for every segment in each range. Replacing duplication with an event sweep should preserve selectivity while cutting setup cost.

### Change

- Replaced `wide_by_seg` duplication with:
  - `wide_by_hi[seg_hi]` start events
  - `active_wide` list updated while iterating segments high -> low
  - expiry when `seg_lo == current_seg`
- Segment loop now visits only currently active wide entries with lower precompute overhead than Opt 1.

### Build Configuration

- Same parity config as V8:
  - `cargo +nightly build --release --bin prime_count_v9 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`

### Benchmark (Max i64, default runtime knobs)

- 2-run alternating sample:
  - V8: `8.43065s`, `8.45025s`
  - V9: `8.34806s`, `8.58985s`
- Additional 3-run alternating sample:
  - V8: `8.38158s`, `8.60953s`, `8.50288s` (median `8.50288s`)
  - V9: `8.34554s`, `8.42132s`, `8.41196s` (median `8.41196s`)

### Combined 5-Run Snapshot (from both samples)

- V8 runs: `8.43065, 8.45025, 8.38158, 8.60953, 8.50288` (median `8.45025s`)
- V9 runs: `8.34806, 8.58985, 8.34554, 8.42132, 8.41196` (median `8.41196s`)

### Result

- **Tentative improvement**: ~`0.45%` median vs V8 in combined sample.
- Variance remains high; needs larger controlled sample for final verdict.

## Opt 3: O(1) Wide Retire Events (Position Map)

### Hypothesis

Opt 2 still used a per-segment linear scan over `active_wide` to retire expired ranges. Replacing this with explicit retire events and O(1) removals should reduce control overhead and variance.

### Change

- Replaced `(b_idx, seg_lo)` active entries + linear expiry scan with:
  - `wide_start_by_hi[seg]` and `wide_retire_by_lo[seg]` event lists
  - `active_wide: Vec<u32>`
  - `active_pos: Vec<usize>` position map for O(1) `swap_remove` retire
- Activation and retirement are now event-driven without scanning all active entries for expiration.

### Build Configuration

- `cargo +nightly build --release --bin prime_count_v9 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`

### Benchmark (Max i64, default runtime knobs)

- First 2-run alternating sample:
  - V8: `8.47953s`, `8.78985s`
  - V9: `8.47342s`, `8.37359s`
- Additional 3-run alternating sample:
  - V8: `8.51435s`, `8.45642s`, `8.38865s`
  - V9: `8.40120s`, `8.48540s`, `8.41309s`

### Combined 5-Run Snapshot

- V8: `8.47953, 8.78985, 8.51435, 8.45642, 8.38865` (median `8.47953s`)
- V9: `8.47342, 8.37359, 8.40120, 8.48540, 8.41309` (median `8.41309s`)

### Result

- **Current best v9 result**: ~`0.78%` median improvement vs V8 in this sample.
- Correctness preserved in all runs.

### Controlled Confirmation Run (11 paired alternating runs)

- Configuration:
  - Same compile parity as V8 (`+nightly`, `-Zbuild-std=std,panic_abort`, target `x86_64-pc-windows-msvc`)
  - Runtime defaults, `LIMIT=9223372036854775807`
  - Alternating sequence: V8, V9 repeated 11 times
- V8 times:
  - `8.35834, 8.42722, 8.45349, 8.47906, 8.47230, 8.45121, 8.52564, 8.47706, 8.45914, 8.34402, 8.38087`
- V9 times:
  - `8.43635, 8.54311, 8.43195, 8.37040, 8.45526, 8.75089, 8.42108, 8.39821, 8.54948, 8.58258, 8.43222`
- Median:
  - V8: `8.45349s`
  - V9: `8.43635s`
  - Delta: `-0.203%` (V9 slightly faster)
- Mean:
  - V8: `8.43894s`
  - V9: `8.48832s` (affected by high-tail V9 outliers)

### Interpretation

- Opt 3 shows a **small median win** but with higher variance and heavier outliers on V9.
- Current evidence supports a modest improvement, but not a robust large gain.

## Opt 4: Event Allocation Micro-Optimizations (FAILED)

### What

- Tried pre-sizing event vectors (`wide_start_by_hi`, `wide_retire_by_lo`) and shrinking `active_pos` from `usize` to `u32` to reduce setup overhead/variance.

### Result

- 5-pair alternating (V8 vs V9) showed regression:
  - V8 median `8.43321s`
  - V9 median `8.45088s`
  - Delta `+0.210%` (slower)

### Verdict

- Reverted. Keep Opt 3 structure without this micro-tuning.

## Opt 5: Remove Redundant AC Clamp Ops (FAILED)

### What

- Removed several `min(..., primes_len - 1)` clamps in AC based on construction invariants.

### Result

- 5-pair alternating (V8 vs V9) regressed significantly:
  - V8 median `8.40712s`
  - V9 median `8.46872s`
  - Delta `+0.733%` (slower)

### Verdict

- Reverted. Even "redundant" control-flow edits harmed codegen/perf in hot paths.

## Opt 6: Retune B_CHUNKS For V9 (SUCCESS)

### Hypothesis

Opt 3 changes AC scheduling pressure; the best B chunking may shift from V8’s default.

### Tests

- V9-only alternating 5x each:
  - `B_CHUNKS=4`: `8.36714, 8.45714, 8.34307, 8.37942, 8.53210` (median `8.37942s`)
  - `B_CHUNKS=8`: `8.43055, 8.37496, 8.46669, 8.39203, 8.43426` (median `8.43055s`)
  - Delta: `-0.606%` in favor of `B_CHUNKS=4`
- V8 default vs V9(`B_CHUNKS=4`) alternating 5 pairs:
  - V8 median `8.43411s`
  - V9 median `8.38798s`
  - Delta `-0.547%` (V9 faster)

### Change

- Updated V9 code default:
  - `B_CHUNKS` fallback: `8 -> 4`

## Opt 7: Retune AC_SEG For V9 (SMALL WIN)

### Hypothesis

With Opt 3 + `B_CHUNKS=4`, AC segment granularity optimum may move from 200K.

### Tests

- V9 alternating 5x each:
  - `AC_SEG=200000`: `8.40651, 8.33901, 8.61753, 8.40768, 8.33255` (median `8.40651s`)
  - `AC_SEG=180000`: `8.35654, 8.35880, 8.43500, 8.42058, 8.35069` (median `8.35880s`)
  - Delta: `-0.568%` in favor of `180000`
- V8 default vs V9(`AC_SEG=180000`) alternating 5 pairs:
  - V8 median `8.43027s`
  - V9 median `8.40017s`
  - Delta `-0.357%` (V9 faster)

### Change

- Updated V9 code default:
  - `AC_SEG` fallback: `200000 -> 180000`

## Current V9 Defaults

- `B_CHUNKS=4` (env override still supported)
- `AC_SEG=180000` (env override still supported)
- Core AC Opt 3 event-sweep + O(1) retire remains active

### 11-Pair Validation With Current V9 Defaults

- Alternating sequence (V8, V9) × 11, Max i64, compile-parity settings.
- V8 times:
  - `8.43000, 8.43112, 8.55356, 8.35818, 8.40158, 8.37742, 8.50000, 8.38638, 8.41258, 8.43186, 8.42527`
- V9 times:
  - `8.59007, 8.35840, 8.41706, 8.46520, 8.30693, 8.37621, 8.47714, 8.35480, 8.59190, 8.60242, 8.38658`
- Median:
  - V8: `8.42527s`
  - V9: `8.41706s`
  - Delta: `-0.097%` (tiny median win)
- Mean:
  - V8: `8.42800s`
  - V9: `8.44788s` (worse due high-tail outliers)

### Current Assessment

- V9 remains slightly faster on median but not robustly better due heavier tail variance.

## Opt 8: Exhaustive Runtime Knob Sweep (Most paths exhausted)

### Tested (V9, Max i64)

- `POOL_MULT`: 2, 3, 4, 5 -> best at `3` (current default)
- `B_THREADS`: 12, 16, 20, 24, 28, 32 -> best at `24` (current default)
- `D_SEG_CAP`: 18, 19, 20, 21, 22 -> best at `20` (current default)
- `D_CHUNKS`:
  - Initial sweep suggested 16/32 close.
  - 7-run alternating `24 vs 32`: median `24` better by ~`0.188%`.
- `B_CHUNKS`:
  - Re-validated `2 vs 4` (5 alternating): `4` better by ~`0.255%` (keep 4).
- `AC_SEG`:
  - Fine sweep (160K..190K) then alternating checks.
  - `175K` did not beat `180K`; `185K` inconclusive and lost in 7-run retest.
  - Keep `180K`.

### Scheduler architecture modes

Single-run check of special modes (with current defaults):
- `DEFAULT`: 8.33320s (best)
- `SEQ_MODE`: 9.22849s
- `PHASE_DB_AC`: 8.87663s
- `PHASE_AC_DB`: 8.79727s
- `PHASE_D_ACB`: 9.03349s
- `PHASE_D_ACB2`: 8.78301s
- `PHASE_D_ACB3_AC8`: 15.79096s
- `PHASE_B_ACD`: 9.48811s

Verdict: default concurrent architecture remains optimal.

## Opt 9: Max-i64 Alpha Retune (SUCCESS)

### Hypothesis

After v9 scheduler/dataflow changes, the high-end Gourdon alpha endpoint may have shifted.

### Exploration

- `ALPHA_Y` sweep around endpoint indicated current `18.5` still good.
- `ALPHA_Z` sweep (`ALPHA_Y=18.5`) showed strong signal:
  - `1.5` faster than `1.3` in alternating runs.

### Validation

- V9 internal alternating (`AZ 1.3 vs 1.5`, 5 runs each):
  - Median `AZ=1.5` improved by ~`0.279%`.
- V8 default vs V9 with `AZ=1.5` (5 pairs):
  - V8 median `8.42595s`
  - V9 median `8.34810s`
  - Delta `-0.924%` (V9 faster)

### Code Change

- Updated V9 alpha table endpoint:
  - `(43.6, 18.5, 1.3)` -> `(43.6, 18.5, 1.5)`

## Opt 10: Combined Defaults Revalidation (SUCCESS)

Current V9 defaults:
- AC Opt 3 event-sweep + O(1) retire
- `B_CHUNKS=4`
- `AC_SEG=180000`
- `D_CHUNKS=24`
- alpha endpoint at max scale `(18.5, 1.5)`

### 11-Pair alternating validation (V8 vs V9)

- V8 median: `8.41433s`
- V9 median: `8.35344s`
- Delta: `-0.724%` (V9 faster)
- Means:
  - V8 `8.40973s`
  - V9 `8.35400s`

### Final Status

- Robust win at Max i64 with current V9 stack.
- Remaining practical paths are exhausted without architectural rewrite.

## Opt 11: Compact AC Hot Metadata Layout (SMALL WIN)

### Hypothesis

AC inner loops repeatedly access per-`b` metadata; shrinking this hot metadata can reduce cache pressure in segment loops.

### Change

- Added compact `HotLookup` representation used only by AC segment loops:
  - `b_term: i32`, `xp: u64`, `l_cur: u32`, `l_max: u32`, `y_boundary_l: u32`, `is_c2: u8`
- Converted from larger `BLookup` before timed AC loop.

### Validation

- 9-pair alternating V8 vs V9:
  - V8 median `8.43784s`
  - V9 median `8.43199s`
  - Delta `-0.069%` (small win)
  - Means: V8 `8.43337s`, V9 `8.41894s`

### Verdict

- Keep change; modest but positive.

## Opt 12: Split AC Wide/Narrow Parallel Loops (FAILED)

### Hypothesis

Splitting wide and narrow paths into separate `par_iter` loops should remove branch/control overhead in the mixed iterator.

### Result

- 5-pair alternating regressed:
  - V8 median `8.42291s`
  - V9 median `8.43256s`
  - Delta `+0.115%` (slower)

### Verdict

- Reverted.

## Exhaustion Summary

Paths now exhausted in V9 without architectural rewrite:
- Scheduler architecture modes (`SEQ_MODE`, all `PHASE_*`)
- Runtime knobs: `POOL_MULT`, `B_THREADS`, `D_SEG_CAP`, `D_CHUNKS`, `B_CHUNKS`, `AC_SEG`
- Max-scale alpha retune (completed)
- AC metadata/layout micro-tuning (only minor gains)
- Loop-structure split variants (regressed)
