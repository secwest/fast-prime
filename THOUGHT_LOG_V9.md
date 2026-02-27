# THOUGHT LOG V9

## 2026-02-27

- User requested a new V9 line of work and asked to leave V8 untouched.
- Selected first V9 optimization target: AC wide `b` scheduling overhead in `compute_ac`.
- Rationale: this is a real code-path inefficiency not requiring algorithm rewrite.

### Analysis Summary

- V8 already exhaustively tuned micro-ops, prefetching, layout, and threading.
- Remaining plausible win is reducing scheduler/no-op overhead in AC segment traversal.
- Existing AC loop scanned all wide entries per segment:
  - many entries inactive in each segment
  - repeated per-segment bound checks and early returns

### Action Taken

- Forked `src/bin/prime_count_v8.rs` to `src/bin/prime_count_v9.rs`.
- Implemented per-segment active wide buckets:
  - computed wide segment span from `xpq_min..xpq_max`
  - built `wide_by_seg[seg]` index lists
  - segment loop now iterates only active wide entries
- Updated banner and AC comment labels from V8 to V9.

### Next Steps

- Build `prime_count_v9`.
- Run correctness check at Max i64.
- Run comparative timing vs V8 (multi-run median).

## 2026-02-27 (continued)

### Build/Run Notes

- `cargo build --release --bin prime_count_v9` failed on stable toolchain due to nightly `-Z` flags in `.cargo/config.toml`.
- Rebuilt successfully with `cargo +nightly build --release --bin prime_count_v9`.

### Initial A/B Timing

- Environment: default runtime knobs, `LIMIT=9223372036854775807`.
- Sequence: V8 -> V9 -> V8 -> V9.
- Results:
  - V8: 8.33240s, 8.56437s
  - V9: 8.42584s, 8.64194s
- Correctness: both binaries returned expected prime count.

### Interpretation

- Wide bucketing did not improve wall time in this first sample.
- Likely cause: added setup/allocation cost and reduced per-segment uniformity may offset fewer no-op wide scans.
- Keep change as Opt 1 baseline for V9; next step should be reducing bucket construction overhead or switching to lighter range-event scheduling.

## 2026-02-27 (compile parity correction)

- User reminder: V9 comparisons must use the same compile-time options documented for V8.
- Confirmed in docs: nightly + `-Zbuild-std=std,panic_abort` + `--target x86_64-pc-windows-msvc`, with Arrow Lake rustflags from `.cargo/config.toml`.

### Rebuild Commands Used

- `cargo +nightly build --release --bin prime_count_v8 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`
- `cargo +nightly build --release --bin prime_count_v9 -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc`

### Re-test Results (Max i64)

- Sequence: V8 -> V9 -> V8 -> V9
- V8: 8.50764s, 8.42613s
- V9: 8.43184s, 8.70700s
- All runs correct.

### Takeaway

- Under compile-parity settings, Opt 1 still does not show improvement.
- Proceed with a lower-overhead wide scheduling variant if continuing V9.

## 2026-02-27 (Opt 2 iteration)

### Implemented

- Reworked AC wide scheduling from duplicated per-segment buckets (Opt 1) to event-sweep active set:
  - `wide_by_hi` start events
  - `active_wide` maintained during reverse segment traversal
  - retire entries when crossing `seg_lo`

### Why

- Opt 1 likely lost performance due to memory/setup overhead from range duplication.
- Event model keeps the "only process active wide b-values" property with lower setup cost.

### Results

- Initial 2-run alternating:
  - V8: 8.43065, 8.45025
  - V9: 8.34806, 8.58985
  - Mixed signal (one strong win, one outlier loss on V9).
- Follow-up 3-run alternating:
  - V8: 8.38158, 8.60953, 8.50288 (median 8.50288)
  - V9: 8.34554, 8.42132, 8.41196 (median 8.41196)
  - Clear positive shift in this sample.

### Current read

- Combined 5-run snapshot suggests ~0.45% median gain for V9.
- Need larger run count and temperature control for confidence.

## 2026-02-27 (Opt 3 refinement)

### Implemented

- Replaced Opt 2's linear expiry scan with event-driven O(1) retire:
  - start events by `seg_hi`
  - retire events by `seg_lo`
  - `active_pos` index map + `swap_remove` for removals

### Reasoning

- Opt 2 still paid `O(active_wide)` retirement scan each segment.
- Event-driven retirement removes only entries that actually expire that segment.

### Results

- 2-run alternating:
  - V8: 8.47953, 8.78985
  - V9: 8.47342, 8.37359
- 3-run alternating:
  - V8: 8.51435, 8.45642, 8.38865
  - V9: 8.40120, 8.48540, 8.41309
- Combined 5-run medians:
  - V8 median: 8.47953
  - V9 median: 8.41309
  - Delta: ~0.78% in favor of V9

### Status

- Opt 3 is currently the strongest V9 candidate.
- Next: run longer controlled benchmark set (e.g., 9-11 paired runs) before locking defaults.

## 2026-02-27 (11-pair validation)

### Method

- Ran 11 alternating pairs (V8 then V9) at Max i64 with default runtime knobs.
- Parsed the `Max i64` line to collect per-run timings.

### Raw Results

- V8: 8.35834, 8.42722, 8.45349, 8.47906, 8.47230, 8.45121, 8.52564, 8.47706, 8.45914, 8.34402, 8.38087
- V9: 8.43635, 8.54311, 8.43195, 8.37040, 8.45526, 8.75089, 8.42108, 8.39821, 8.54948, 8.58258, 8.43222

### Summary

- Median:
  - V8 8.45349
  - V9 8.43635
  - V9 advantage: ~0.203%
- Means:
  - V8 8.43894
  - V9 8.48832
- Observation: V9 has larger tail outliers (8.75, 8.58, 8.55), suggesting sensitivity/variance despite slightly better median.

### Decision

- Keep Opt 3 as active V9 direction.
- Continue searching for stability improvements that reduce V9 tail variance.

## 2026-02-27 (continued tuning after 11-pair run)

### Attempt A: Event-allocation micro-optimizations

- Change:
  - pre-sized event vectors by per-segment counts
  - `active_pos` changed from `usize` to `u32`
- Result:
  - 5-pair alternating gave median regression (~+0.21%).
- Action:
  - Reverted.

### Attempt B: Remove redundant AC clamp mins

- Change:
  - removed several `min(..., primes_len-1)` clamps in AC where invariants seemed to guarantee bounds.
- Result:
  - 5-pair alternating regressed strongly (~+0.73%).
- Action:
  - Reverted.

### Attempt C: Retune B chunking for V9

- Observation:
  - V9 behaved better at lower `B_CHUNKS` than V8 defaults.
- Alternating V9-only test:
  - `B_CHUNKS=4` median 8.37942
  - `B_CHUNKS=8` median 8.43055
  - ~0.6% improvement with 4.
- Cross-check vs V8:
  - V8 median 8.43411
  - V9(B4) median 8.38798
  - ~0.55% advantage to V9.
- Action:
  - Set V9 default `B_CHUNKS` fallback to 4.

### Attempt D: Retune AC_SEG for V9

- Alternating V9 test:
  - `AC_SEG=180000` median 8.35880
  - `AC_SEG=200000` median 8.40651
  - ~0.57% in favor of 180K.
- Cross-check vs V8:
  - V8 median 8.43027
  - V9(SEG180) median 8.40017
  - ~0.36% advantage to V9.
- Action:
  - Set V9 default `AC_SEG` fallback to 180000.

### Current state

- Active V9 stack:
  - Opt 3 AC wide event-sweep + O(1) retire
  - `B_CHUNKS` default = 4
  - `AC_SEG` default = 180000
- Overall: small but repeatable median gains, still with non-trivial run-to-run noise.

## 2026-02-27 (11-pair revalidation after new defaults)

### Result

- V8 median: 8.42527
- V9 median: 8.41706
- Delta: ~0.10% in favor of V9 (very small)
- Means:
  - V8: 8.42800
  - V9: 8.44788 (hurt by V9 outliers 8.590, 8.592, 8.602)

### Interpretation

- New defaults keep V9 at slight median advantage, but variance remains the major issue.
- Next optimization focus should target stability/tail mitigation, not raw median only.

## 2026-02-27 (exhaustive continuation)

### Runtime/path sweeps performed

- `POOL_MULT` sweep: confirmed `3` remains best.
- `B_THREADS` sweep: confirmed `24` remains best.
- `D_SEG_CAP` sweep: confirmed `20` remains best.
- `D_CHUNKS` sweep and alternating tests:
  - long alternating suggested `24` marginally beats `32`.
  - changed v9 default to `24`.
- Rechecked `B_CHUNKS`:
  - `2` looked good in single run but lost to `4` in alternating.
  - kept `4`.
- Fine `AC_SEG` retune:
  - tested 160K..190K.
  - 175/185 not robustly better than 180 in longer alternating checks.
  - kept `180000`.

### Architecture mode exhaustion

- Tested `SEQ_MODE` and all `PHASE_*` paths again.
- All were slower than default; default concurrent mode still best.

### Alpha endpoint discovery

- With v9 stack, `ALPHA_Z` endpoint at max scale shifted:
  - `AZ=1.5` consistently beat `AZ=1.3` when `AY=18.5`.
- Baked into table by changing final point to `(43.6, 18.5, 1.5)`.

### Post-change robust validation

- 11 alternating pairs (V8 vs V9 defaults):
  - V8 median 8.41433
  - V9 median 8.35344
  - ~0.724% median improvement
  - Means also improved for V9 (8.35400 vs 8.40973), indicating reduced outlier damage.

### Conclusion at this stage

- Practical optimization paths in current v9 architecture are largely exhausted.
- Remaining improvements likely require deeper algorithmic/architectural changes (V10-class work), not incremental tuning.

## 2026-02-27 (final continuation pass)

### Additional AC micro-paths tried

- Compact AC metadata (`HotLookup`) for segment loop:
  - Kept.
  - 9-pair V8 vs V9: median edge ~0.07%, mean improved for V9.
- Split wide/narrow into separate parallel loops:
  - Regressed in 5-pair check (~+0.12%).
  - Reverted.

### State after final pass

- Active improvements remain:
  - AC Opt 3 event-sweep + O(1) retire
  - Compact AC `HotLookup`
  - `B_CHUNKS=4`
  - `AC_SEG=180000`
  - `D_CHUNKS=24`
  - alpha endpoint update to `(18.5, 1.5)` at max scale

### Practical stopping point

- Within the current v9 algorithm/scheduler, remaining incremental paths have been tested and either adopted or exhausted.
- Next non-trivial gains require a deeper architecture shift (new algorithm/dataflow design).
