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

## 2026-03-06 (continuation)

- Revalidated key B/D knobs under current defaults and compile profile.
- `B_CHUNKS=2` remains slightly better than `4` in a fresh 22-run alternation.
- `D_CHUNKS=20` vs `24` remained split/noisy (median/mean disagreement), so no safe retune.
- `D_AUTO_CHUNK_SELECT=1` vs `0` also remained mixed; kept fixed default mode.

### New robust win

- Re-swept `D_SEG_MIN_CAP` and found a strong candidate at `14`.
- 22 alternating pairs (`14` vs `17`) favored `14` on both median and mean by ~0.7-0.75%.
- Cross-check versus V9 (22 alternating) still favored V10 with this setting.
- Promoted `D_SEG_MIN_CAP` default fallback from `17` to `14`.

### Current read

- Small but repeatable gain remains possible through scheduler boundary retuning.
- Deeper loop rewrites in B/D still mostly regress; default tuning remains the safest win path.

## 2026-03-06 (later continuation)

- After landing `D_SEG_MIN_CAP=14`, rechecked local neighborhood (`13/14/15/17`) with longer alternations.
- `14` stayed robust vs `13`, while `14` vs `15` was noisy and split across windows; `15` did not hold vs `17`.
- Kept `D_SEG_MIN_CAP=14`.

### Follow-on AC retune

- Re-swept AC with the new D baseline and found `AC_SEG=170000` now edges `160000`.
- Confirmed with both 22-run and 30-run reversed-order alternations; `170000` won on median and mean in the longer window.
- Updated default/runtime top tier to `AC_SEG=170000`.

### Result

- New combined defaults: `AC_SEG=170000`, `D_SEG_MIN_CAP=14`, `B_CHUNKS=2`, `D_CHUNKS=24`.
- Cross-check against V9 remained favorable (~0.65% median, ~0.9% mean in latest 22 alternating run).

## 2026-03-06 (latest continuation)

- Ran a full post-retune exhaustion sweep to ensure no immediate follow-up wins were missed.
- `B_CHUNKS=1` vs `2` landed as a practical tie on median; mean slightly favored `2`, so kept `2`.
- Rechecked `AC_PAR_MIN` with the new defaults; nonzero (`192`) regressed again.
- Reconfirmed `D_SEG_CAP=20`, `D_CHUNKS=24`, and `POOL_MULT=3` as best local settings in fresh sweeps.

### Reverted code attempts

- `compute_b` pre-sum `pi_fast` unchecked variant: slight regression in 22-run A/B.
- AC mixed-boundary unrolled crossover scalarization: slight regression in 22-run A/B.

### Current state

- No additional retained changes beyond `AC_SEG=170000` and `D_SEG_MIN_CAP=14`.
- V10 remains in a narrow-noise regime where many micro-edits regress or wash out.

## 2026-03-06 (latest later continuation)

- Found a new D-side tuning path that had not been exhausted: the sparse `ValidM` index stride.
- Added cached runtime support for `VM_STRIDE` and swept several granularities.
- Both `24` and `32` beat the old `64` default in long alternating runs.
- A longer direct `24` vs `32` comparison favored `32`, so promoted `VM_STRIDE=32`.

### Result

- This is a retained code-level/default improvement, not just another noisy scheduler tweak.
- Latest tuned V10 with `VM_STRIDE=32` widened the V9 gap to roughly `-1.33%` median / `-1.70%` mean in the fresh 22-run cross-check.

### Additional follow-up

- Tested a second-order sparse-index knob, `VM_LOOKAHEAD`, after landing `VM_STRIDE=32`.
- Results were mixed (`1` helped median slightly, `2` helped mean), so kept default lookahead unchanged at `2`.

## 2026-03-06 (latest final continuation)

- Revalidated the `VM_STRIDE` neighborhood after landing the new default.
- Fresh single-run sweep still pointed clearly at `32`; nearby values (`20/24/28/36`) did not beat it.

### Additional checks

- Re-ran AC segment tuning because the D-side `VM_STRIDE` change could have shifted contention balance.
- `AC_SEG=180000` had a strong single-run signal but failed in 22 alternating runs.
- `AC_SEG=150000` also failed to beat the current `170000` default in long A/B.

- Probed `CompactPi` coarse stride at compile time:
  - `128` was clearly worse in a single run.
  - `512` looked interesting in one run, but not enough to override the previously retained `256` layout.
  - Reverted to `PI_STRIDE=256`.

### Current state

- `VM_STRIDE=32` remains the only new retained improvement from this continuation wave.
- Everything else tested after that either regressed, stayed mixed, or lacked enough evidence to replace the current defaults.

## 2026-03-07 (continuation)

- Returned to `VM_LOOKAHEAD` because the first 22-run result was still split.
- A longer 30-run reversed-order comparison resolved it in favor of `1`.
- Promoted `VM_LOOKAHEAD=1` as the new default alongside `VM_STRIDE=32`.

### Current read

- The `ValidM` sparse-index path yielded two real improvements in sequence:
  - narrower index stride (`32`)
  - tighter search window (`1`)
- This is now one of the few D-side areas still capable of moving total wall time in a repeatable way.

## 2026-03-07 (later continuation)

- Re-ran `VM_STRIDE` after landing `VM_LOOKAHEAD=1`.
- The first sweep was noisy, but a longer reversed direct comparison resolved `24` over `32`.
- Promoted `VM_STRIDE=24` as the new default.

### Current read

- The sparse-index path has now produced three sequential retained tuning wins:
  - `D_SEG_MIN_CAP=14`
  - `VM_LOOKAHEAD=1`
  - `VM_STRIDE=24`
- This remains the most productive optimization surface left in V10.

## 2026-03-07 (latest continuation)

- Reopened the joint sparse-index surface to see if the new `VM_STRIDE=24` default implied a different `VM_LOOKAHEAD` or mixed pair.
- Short runs produced several tempting candidates (`lookahead=4`, `(32,2)`, `(16,2)`), but longer reversed comparisons pushed the result back toward the current `(24,1)` default.

### Additional checks

- Re-ran `D_SEG_MIN_CAP` under the new sparse-index defaults; the quick sweep moved, but long A/B did not confirm a better replacement for `14`.
- Rechecked `D_SEG_CAP`, `D_CHUNKS`, and `B_CHUNKS`; all current defaults still look locally optimal.

### Current state

- No new retained change in this continuation.
- The current tuned baseline remains stable, and the remaining local search space is increasingly dominated by noise.

## 2026-03-07 (latest later continuation)

- Reopened the joint `(VM_STRIDE, VM_LOOKAHEAD)` surface from the new `24/1` baseline.
- Several short-window candidates appeared (`lookahead=4`, `(32,2)`, `(16,2)`), but the longer reversed comparisons again pushed the result back to the current defaults.

### Reverted code path

- Tried splitting `ValidM` search into a separate `Vec<u32>` of `m` values so the bucket `partition_point` step would stop touching the full 16-byte records.
- The alternating run regressed, so the code was reverted.

### Current state

- No new retained change in this continuation.
- The sparse-index path still looks like the only remaining productive area, but it is increasingly dominated by window-order noise rather than stable wins.

## 2026-03-11 (continuation)

- Returned to D with two code-level paths instead of more knob churn.
- The monotone Type 1 window idea was cheap to test but stayed mixed in A/B, so it was reverted.
- A local-accumulator rewrite for `phi[b]` / `coeff[b]` looked plausible, but it exposed a correctness failure in the default concurrent/global-pool flow even though `SEQ_MODE` and dedicated-D runs were correct.
- Reverted that path immediately rather than treat a concurrency-sensitive micro-optimization as acceptable.

### Retained change

- Found a safer D-side win in the sparse-index lookup itself:
  - when `min_m` and `max_m` fall in the same bucket, reuse the same `ValidM` slice for both searches
  - longer A/B runs confirmed the gain after order reversal
- This is a real code-path improvement, not a default retune.

### Current read

- The sparse-index area is still productive, but only when the optimization preserves the existing execution model and stays structurally simple.
- The more invasive D hot-loop rewrites are now more likely to surface correctness or concurrency hazards than to deliver a safe win.

## 2026-03-11 (later continuation)

- Tried to refine the retained same-bucket D path rather than opening another broad knob sweep.
- Three follow-up micro-edits all failed in balanced A/B:
  - tail-only second `partition_point` in the same-bucket path
  - unchecked bucket-slice helper for `vm_index` / `ValidM`
  - manual reverse index walk over `ValidM` instead of `iter().rev()`

### Read on the failures

- The sparse-index lookup is sensitive enough that even seemingly obvious "less work" rewrites can lose once codegen and cache behavior shift.
- The first post-build single run was again misleading on two of these paths; long alternating windows remain mandatory.
- This reinforces the older conclusion from the broader logs: unsafe indexing / iterator rewrites are not automatically wins here, and the compiler already does most of the easy work.

### Compile-side recheck

- Revalidated that the documented compile parity from V8/V9 is still active:
  - nightly + `-Zbuild-std=std,panic_abort`
  - `target-cpu=native`
  - `-Ztune-cpu=arrowlake`
  - fat LTO, single codegen unit
- So there is no obvious repo-local compile flag left to mine.

### Current state

- No new retained change in this continuation.
- The last retained same-bucket sparse-index fast path remains the current best V10 code state.

## 2026-03-11 (instrumented continuation)

- Stopped guessing inside the D sparse-index path and added an env-gated measurement hook instead.
- `D_VM_STATS=1` now reports how often Type 1 stays in one bucket, how often it crosses buckets, how often the result span is empty, and how large the searched/result slices really are.

### What the stats said

- Same-bucket queries dominate (`71.4%`), but their average search window is only `4.1` `ValidM` items and the average result span is only `0.4`.
- Cross-bucket queries are fewer (`28.6%`) but their average result span is huge (`297.5` items), so lookup-bound and leaf-bound work are no longer aligned.
- About half of all sparse-index lookups end up empty (`50.1%`).

### Read on that data

- The retained same-bucket fast path is probably close to the end of its useful search-space.
- Most of the remaining time pressure is no longer in finding the boundaries for same-bucket windows; it is in the much larger cross-bucket leaf spans.
- That makes another same-bucket micro-rewrite hard to justify without fresh evidence.

### Follow-up test

- Tried one data-driven near-cross-bucket rewrite:
  - for bucket delta `<= 2`, collapse the two boundary probes into one contiguous `ValidM` window
- Result:
  - looked better in the first 22-run alternating window
  - lost after order reversal
  - reverted immediately

### Current state

- Retained only the `D_VM_STATS` diagnostic path from this continuation.
- No new production V10 speedup from this pass.
- If D tuning continues, the next rational move is either:
  - richer cross-bucket instrumentation/histograms, or
  - a larger leaf-iteration experiment rather than another lookup micro-edit.

## 2026-03-11 (leaf-loop continuation)

- Used the new D stats to stop chasing boundary-lookup trivia and move directly into the heavier leaf loops.
- Before touching code, rechecked `VM_STRIDE` under the retained same-bucket fast path in case the new lookup behavior had shifted the old optimum.
- It had not: `VM_STRIDE=24` still beat the nearby larger-bucket candidates in a fresh single-run sweep.

### Retained change

- Hoisted stable per-`b` values out of the hottest D loops:
  - `phi[b]` to a local `phi_b`
  - `coeff[b]` to a single mutable slot reference per `b`
  - repeated `prime` casts
  - Type 1 `lpf` comparison onto `u16`

### Why this path was different from the earlier failed accumulator rewrite

- The older failed rewrite changed update structure by accumulating locally and writing back later.
- This pass kept the write timing intact; it only removed repeated indexing/casting from the inner loops.
- That distinction mattered: Max-i64 correctness stayed intact in the default concurrent/global-pool flow.

### Validation

- 22 alternating, candidate first: about `-0.78%` median and `-0.90%` mean vs retained baseline.
- 22 alternating, reversed order: about `-0.62%` median and `-0.56%` mean.
- 22 alternating vs V9: about `-0.93%` median and `-0.94%` mean.

### Current state

- This continuation produced a real retained V10 speedup.
- `D_VM_STATS` remains available for later guided work, but the code is now left on the new leaf-loop-hoisted baseline.

## 2026-03-11 (check-in synthesis)

This check-in closes a coherent D-focused optimization wave. The important story is not just the final retained win, but why the search moved the way it did.

### 1) Start point

- The retained baseline going into this wave was the same-bucket sparse-index fast path.
- That win was real because it improved the lookup path without disturbing the surrounding execution model.
- The working assumption at that point was that the remaining D opportunity was still somewhere in sparse-index lookup specialization.

### 2) What was tried first and why it failed

- Several "obvious" same-bucket follow-ups were tested first because they were the smallest deltas from the retained win:
  - tail-only second `partition_point`
  - unchecked bucket-slice helper
  - manual reverse `ValidM` walk
- All three lost in balanced A/B.
- The main lesson was that the lookup code had become sensitive enough that tiny source-level rewrites were now dominated by codegen/cache effects rather than theoretical instruction-count savings.

### 3) Why instrumentation became necessary

- At that point the search space was producing too much window-order noise to trust intuition.
- `D_VM_STATS` was added specifically to answer whether the remaining cost was still in lookup-boundary work or had moved into the leaf iteration body.
- The measurements made the next decision clear:
  - same-bucket lookups were already tiny
  - cross-bucket spans were fewer but much heavier
- That changed the optimization target from "boundary search" to "leaf-loop cost per accepted range."

### 4) What the instrumentation invalidated

- A near-cross-bucket combined-window rewrite was still worth one data-backed attempt because the bucket-delta histogram showed plenty of `delta=1/2` traffic.
- It failed order reversal and was reverted.
- That result mattered because it ruled out the last easy-looking lookup specialization that still had a plausible workload argument behind it.

### 5) Why the retained leaf-loop change was acceptable when the earlier accumulator rewrite was not

- The old accumulator attempt changed update structure and turned out to be unsafe in the default concurrent/global-pool flow.
- The retained hoisting change is qualitatively different:
  - it keeps update timing intact
  - it does not change cross-chunk semantics
  - it only removes repeated indexing/casting from the hot loops
- That is why it was worth revisiting the leaf loops even after the previous "local accumulator" failure.

### 6) Choices made at check-in time

- Kept:
  - same-bucket sparse-index fast path
  - `D_VM_STATS` instrumentation
  - leaf-loop hoisting of stable per-`b` values
- Rejected:
  - same-bucket tail-search refinement
  - unchecked bucket-slice helper
  - manual reverse `ValidM` walk
  - combined near-cross-bucket window
  - fresh `VM_STRIDE` retune away from `24`

### 7) Current read going forward

- The D lookup surface is not fully exhausted, but the cheap local wins there are mostly gone.
- The most credible future gains are now either:
  - better-guided cross-bucket/leaf-loop work using the retained instrumentation, or
  - a larger structural rethink of D iteration, not another cosmetic rewrite of the current sparse-index boundary logic.

## 2026-03-11 (post-push continuation)

- Continued from the pushed leaf-loop-hoisted baseline instead of stacking work on an unpushed branch.
- The first goal in this continuation was to see whether the new baseline had shifted any previously settled local optima before opening another big code path.

### What was tried and rejected

1. Type 2 local `primes[l]` / `prime_recip[l]` hoist
- Looked like a natural extension of the D leaf-loop win.
- Failed order reversal, so it was reverted.

2. Cross-bucket sparse-index de-duplication
- Fixed a real source-level duplication, but the measured gain was effectively zero.
- Reverted instead of keeping a complexity-neutral tie.

3. AC C2/A inner-loop split
- This was the only AC-side structural try in the continuation.
- It regressed immediately and badly on the first Max-i64 run, so it was dropped without further spending.

4. `D_SEG_MIN_CAP` recheck
- The single-run sweep suggested `16` might have displaced `14`.
- Balanced A/B said no; kept `14`.

### Retained change

- Re-opened `B_CHUNKS` on the new baseline and found the old `2` default was no longer best.
- `B_CHUNKS=4` won on median in both orderings and held up on a longer 30-run alternating window.
- This is a default/runtime-tuning change, not a code-structure rewrite.

### Why that choice was acceptable despite one mixed short window

- The first two 22-run windows gave:
  - consistent median improvement for `4`
  - one tiny mean loss in reversed order (`+0.021%`), which was too small to trust by itself
- The longer 30-run window resolved that ambiguity and restored both median and mean advantage for `4`.
- That made the retune stronger than the earlier rejected micro-edits in this same continuation.

### Current state

- The pushed baseline has now moved again, this time by a retuned `B_CHUNKS` default from `2` to `4`.
- The D-side code win from the previous continuation still stands unchanged.
- The most obvious cheap post-hoist D and AC source rewrites from this continuation have now been tested and rejected.

## 2026-03-13 (D branch-pruning continuation)

- Continued from the pushed `B_CHUNKS=4` baseline and rechecked the current D stats first.
- The stats still pointed at leaf work, not lookup-boundary work:
  - same-bucket queries were still about `71%`, but cross-bucket ranges were much
    heavier with about `297.5` result items on average
  - that made another tiny boundary-search rewrite hard to justify

### Retained change

- Removed interval/order branches that the existing leaf windows already prove away:
  - `xpm` / `xpq` are already constrained to `[low, high)`
  - Type 2 wheel positions remain monotone in the existing reverse walk
- The code change stayed deliberately small:
  - no change to update timing
  - no change to chunk correction semantics
  - mirrored in the serial fallback so the two D paths stay behaviorally aligned

### Validation

- Full built-in correctness sweep still passed through `Max i64`.
- A quick instrumented sanity check improved D from about `5.95s` to `5.91s`.
- A 12-run alternating A/B on `Max i64` (both orders, 6 candidate + 6 baseline)
  was strong enough to keep:
  - candidate median `8.34409s`, mean `8.34551s`
  - baseline median `8.36555s`, mean `8.36550s`
  - about `-0.26%` median and `-0.24%` mean

### Current read

- This is another small but real D-side leaf-loop win, not a new scheduling or
  tuning change.
- The remaining space still looks like "leaf-body cost under heavy cross-bucket
  spans," but the bar for keeping future source rewrites should stay high unless
  they beat this result under balanced A/B.

## 2026-03-13 (post-pruning retune continuation)

- After landing the branch-pruning win, the next question was whether the new
  D baseline had shifted any of the older scheduler or sparse-index optima.
- A small helper-level code try was tested first:
  - 4-way unrolled shared popcount-range helper for `BitSieve::count()` and
    `count_delta()`
  - it immediately regressed to `9.22871s` on the first full `Max i64` sweep
  - reverted without further spending

### What was rechecked and rejected

1. `D_CHUNKS`
- Fresh single-run sweep put `24` back on top.
- No reason to reopen the chunking default.

2. `B_CHUNKS`
- `4` and `2` were again very close in singles.
- `4` still held the edge, so there was no new retune there.

3. `AC_SEG=150000`
- Looked excellent in a short single-run sweep.
- Lost clearly in a 12-run alternating A/B against the retained `170000`.
- This was another example of a tempting short-window result collapsing under
  balanced order control.

4. `VM_LOOKAHEAD=3`
- Same pattern as `AC_SEG=150000`:
  - short sweep looked promising
  - 12-run alternating comparison lost to the retained default `1`

### Retained change

- `VM_STRIDE` finally moved again on the new baseline.
- Fresh singles made `16` the most credible candidate, with `32` the only
  nearby alternative still worth checking.
- Validation came out clean:
  - 12-run alternating (`16` vs `24`) favored `16` on both median and mean
  - 12-run alternating (`16` vs `32`) also favored `16`

### Current state

- The continuation retained one new default:
  - `VM_STRIDE: 24 -> 16`
- This is consistent with the current read that the remaining wins are still
  buried in the D leaf path, but also shows that the branch-pruned baseline did
  move at least one previously settled sparse-index tuning point.

## 2026-03-13 (post-`VM_STRIDE` continuation)

- The next question after landing `VM_STRIDE=16` was whether the nearby
  sparse-index surface had shifted again.
- A small matrix over `(VM_STRIDE, VM_LOOKAHEAD)` produced one suspiciously good
  short-run outlier at `(12,1)`, but the follow-up round-robin over
  `10/12/14/16` flattened that back into noise.
- No further sparse-index default moved in this pass.

### Why the search moved to AC

- `SHOW_TIMING` still had AC as the longest phase after the recent D wins.
- That made `AC_PAR_MIN` the next cheapest credible lever:
  - purely a runtime threshold
  - no structural change to AC loops
  - easy to validate under alternating A/B

### What was found

- `AC_PAR_MIN=64` first looked like a small improvement over `0`, and a
  balanced 12-run check did keep a narrow edge.
- But the nearby sweep reopened the question and made `32` the stronger
  candidate against the actual default.
- The important comparison was not just `32` vs `64`; it was:
  1. does the candidate beat `0` cleanly?
  2. does it still look acceptable head-to-head against the other plausible
     candidate?

- `32` answered that better than `64`:
  - stronger win vs `0`
  - better median story in the direct `32` vs `64` checks

### Current state

- Retained one new AC-side default from this continuation:
  - `AC_PAR_MIN: 0 -> 32`
- This is not an AC algorithm rewrite; it is a small scheduling-threshold retune.
- The broader pattern still holds:
  - most apparent single-run wins collapse under balanced order control
  - the surviving gains continue to come from narrow, well-validated runtime or
    hot-loop changes rather than broad rewrites

## 2026-03-13 (post-`AC_PAR_MIN` continuation)

- After landing `AC_PAR_MIN=32`, the next obvious check was whether the AC-side
  segmentation or the AC/B contention balance had shifted.

### What was rechecked

1. `AC_SEG`
- Fresh singles on the new baseline put `170000` back on top.
- No reason to reopen the AC segmentation default.

2. `B_CHUNKS`
- This one did move again.
- The important context is that the AC threshold retune changed how much
  concurrency pressure AC and B place on each other at the top end.
- A fresh sweep made `6` the strongest high-end candidate, and a 12-run
  alternating check against the retained `4` kept a small but consistent edge.

### Why it was kept

- The `Max i64` A/B win was modest, but it went the right direction on both
  median and mean.
- The extra spot checks did not reveal a downside at smaller nearby scales:
  - `1e18` was effectively flat
  - `1e17` leaned slightly toward `6`
- That made this a reasonable top-tier heuristic retune rather than a
  scale-specific fluke.

### Current state

- Retained one new runtime-tuning change from this continuation:
  - top-end `B_CHUNKS: 4 -> 6`
- At this point the recent optimization wave has now moved three live defaults:
  - `VM_STRIDE: 24 -> 16`
  - `AC_PAR_MIN: 0 -> 32`
  - high-end `B_CHUNKS: 4 -> 6`

## 2026-03-13 (high-end tier follow-up)

- After the top-end `B_CHUNKS` retune, the next sensible target was the runtime
  table itself rather than another global default.
- The key distinction in this pass was scale:
  - `Max i64` and `1e18` still wanted the same `AC_SEG`
  - `1e17` did not

### What moved

- The `1e17` AC segment size finally moved from `190000` to `180000`.
- The win was small, but it survived a longer balanced A/B at that scale.

### What did not move

1. `1e18 AC_SEG`
- A short single-run sweep made `150000` look promising.
- The longer alternating check reversed that and kept `170000`.

2. `1e17 D_CHUNKS`
- `16` looked good in singles once `AC_SEG=180000` was in place.
- Balanced A/B said no and kept `24`.

### Current state

- Retained one new table-level runtime change from this continuation:
  - `1e17 AC_SEG: 190000 -> 180000`
- The recent optimization wave has now changed four live runtime choices:
  - `VM_STRIDE: 24 -> 16`
  - `AC_PAR_MIN: 0 -> 32`
  - high-end `B_CHUNKS: 4 -> 6`
  - `1e17 AC_SEG: 190000 -> 180000`

## 2026-03-14 (tier split follow-up)

- The next pass stayed in the runtime table instead of going back to source
  rewrites.
- Two questions were tested:
  1. does `AC_PAR_MIN` want to split by scale?
  2. does the `1e15..1e16` row really want the same `B_CHUNKS=6` as the
     higher-end tiers?

### What did not move

1. Scale-aware `AC_PAR_MIN`
- `1e17` singles briefly suggested `0` might beat the new global `32`.
- The longer 20-run alternating check said no and kept `32`.

2. `1e17 B_CHUNKS`
- `10` looked attractive in singles after the `1e17 AC_SEG` retune.
- It was too unstable under A/B and was dropped.
- `4` was also checked directly and lost clearly.

3. Aggressive `1e16` row package
- A combined `AC_SEG=220000`, `B_CHUNKS=12`, `D_CHUNKS=12` row looked fast in
  singles.
- The package lost on median against the current row, and even the `AC_SEG`
  piece by itself failed in a direct A/B.

### Retained change

- The surviving move was smaller and cleaner:
  - the `1e15..1e16` row wants `B_CHUNKS=4`, not `6`
- It held at both scales that share the row:
  - strong enough at `1e16`
  - still favorable in a direct `1e15` spot check

### Current state

- Retained one new table-level runtime change from this continuation:
  - `1e15..1e16 B_CHUNKS: 6 -> 4`
- The runtime table has now split in a more coherent way by scale:
  - top end keeps `B_CHUNKS=6`
  - the `1e15..1e16` row moves back to `4`

## 2026-03-14 (low-tier continuation)

- After splitting the `1e15..1e16` row, the next check was whether that row had
  shifted enough to justify more local retunes.
- It had not:
  - `AC_PAR_MIN` did not beat the retained global `32`
  - `AC_SEG=220000` again looked tempting in singles and again lost in A/B
  - `D_CHUNKS=20` stayed put

### Why the search moved lower

- The updated `1e15..1e16` row no longer looked like the best place to spend.
- The `x < 1e15` row still carried the older `B_CHUNKS=8` default, which was
  now out of line with the rest of the runtime table.

### What was found

- `1e14` immediately showed `B_CHUNKS=4` beating `8` by a large margin.
- The result held at `1e13` and remained favorable at `1e12`.
- The other row knobs did not show a case for change:
  - `D_CHUNKS=24` stayed best at `1e14`
  - `AC_SEG` singles were too noisy and not needed once the B result was so clear

### Current state

- Retained one new low-tier runtime change from this continuation:
  - `x < 1e15 B_CHUNKS: 8 -> 4`
- The runtime table now has a much cleaner B-chunk story by scale:
  - low and mid tiers use `4`
  - the top two tiers use `6`

## 2026-03-14 (low-tier AC threshold continuation)

- After the low-tier `B_CHUNKS` split, the next likely question was whether the
  AC threshold should split there too.
- The first stop was the `1e15..1e16` row:
  - `AC_PAR_MIN=64` did not beat the retained `32`
  - `AC_SEG=220000` still failed again
  - `AC_PAR_MIN=0` also lost
- That ruled out another change in the mid row.

### Why the search moved lower again

- The low tier had already shown that it does not always want the same B-side
  settings as the rows above.
- That made it plausible that the AC threshold might also want a low-tier split.

### What was found

- The low-tier sweeps at `1e14` and `1e13` both pushed the best-looking values
  upward into the `48..64` range.
- Balanced checks against the retained `32` showed only small gains, but they
  stayed directionally consistent at:
  - `1e14`
  - `1e13`
  - `1e12`

### Current state

- Retained one new low-tier runtime change from this continuation:
  - `x < 1e15 AC_PAR_MIN: 32 -> 64`
- `RuntimeTuning` now carries `ac_par_min` explicitly instead of relying on one
  global fallback for every scale.

## 2026-03-14 (low-tier D continuation)

- After landing the low-tier `AC_PAR_MIN=64` split, the next question was
  whether that row had shifted enough to justify further tuning.
- `1e14` said "not really":
  - `AC_SEG=280000` did not beat `200000`
  - `D_CHUNKS=16` did not beat `24`

### Why the search moved to `1e13`

- The `1e14` row looked stable, but the low tier still spans several decades.
- That made a deeper split more plausible than another whole-row retune.

### What was found

- `1e13` singles suggested two things:
  - `B_CHUNKS=8` looked better than `4`
  - `D_CHUNKS=12` looked slightly better than `24`
- The first one was another false positive:
  - long A/B put `B_CHUNKS=4` clearly back in front
- The second one held, but only barely.

- The deciding extra checks were:
  - `1e12`: still favored `D_CHUNKS=12`
  - `1e11`: pushed back toward `24`

### Current state

- Retained one new sub-tier runtime change from this continuation:
  - `1e12..1e13 D_CHUNKS: 24 -> 12`
- The low end is now split more precisely:
  - `1e14` stays on the broader low-tier row
  - `1e12..1e13` gets a smaller D chunk count
  - `1e11` and below stay on the original low-tier D setting

## 2026-03-14 (bottom-row B continuation)

- Continued from the new `1e12..1e13` D split and re-opened the remaining
  `x < 1e12` row at `1e11`.
- The first singles looked like a classic low-end trap again:
  - `AC_SEG=160000`
  - `B_CHUNKS=2`
  - `D_CHUNKS=8`
  all appeared attractive in one-shot runs.

### What held up

- Balanced bidirectional checks pushed `AC_SEG=160000` back out immediately.
- Both `B_CHUNKS=2` and `D_CHUNKS=8` survived against the old bottom row, but
  the B-side move was the cleaner one.
- A combined (`B=2`, `D=8`) package looked even better against the old row, but
  direct A/B showed it was not actually the best setting.

### How it was resolved

- Direct `B_CHUNKS=2` vs `D_CHUNKS=8` checks at both:
  - `1e11`
  - `1e10`
  put `B_CHUNKS=2` clearly in front.
- That narrowed the retained move to the B scheduler only; the lower-row
  `D_CHUNKS=24` setting stays put.

### Current state

- Retained one more low-end runtime change from this continuation:
  - `x < 1e12 B_CHUNKS: 4 -> 2`
- The low end now resolves as:
  - `1e12..1e13`: `B_CHUNKS=4`, `D_CHUNKS=12`
  - `1e11` and below: `B_CHUNKS=2`, `D_CHUNKS=24`

## 2026-03-14 (`1e12..1e13` B continuation)

- After splitting off the bottom row, the next obvious question was whether the
  neighboring `1e12..1e13` row still wanted `B_CHUNKS=4`.
- Short sweeps reopened several knobs at once:
  - `1e13` again pointed hard toward `B_CHUNKS=2`
  - `1e12` threw off several tempting single-run false positives (`B=8`,
    `AC_SEG=240000`, `AC_PAR_MIN=16`)

### What held up

- Long bidirectional A/B removed almost all of the extra noise:
  - the AC segment candidates collapsed back to flat or worse
  - the AC threshold candidates also collapsed
  - the D-side nudge (`8` vs `12`) stayed too small and split to keep
- The only robust move left was the B-side one.

### Why it was kept for the whole row

- `1e13` kept a large, clean win for `B_CHUNKS=2`.
- A longer `1e12` validation showed the same direction, just smaller.
- That was enough to move the whole `1e12..1e13` row instead of introducing yet
  another narrower split.

### Current state

- Retained one more runtime-table change from this continuation:
  - `x >= 1e12 B_CHUNKS: 4 -> 2`
- The low/mid-low B story is now:
  - `1e14`: still on `4`
  - `1e12..1e13`: now on `2`
  - `1e11` and below: already on `2`
