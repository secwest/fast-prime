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
