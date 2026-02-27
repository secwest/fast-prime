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
