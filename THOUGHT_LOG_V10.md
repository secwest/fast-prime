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
