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

## Conclusion

V10 scheduling redesign did not produce a robust improvement. It gives small median gains but introduces heavier outliers. V9 remains the safer baseline for consistent performance.
