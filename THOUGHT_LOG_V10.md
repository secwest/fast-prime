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

### Current interpretation

- This V10 scheduler change is experimentally interesting but not a reliable production improvement.
- Practical path from this design is exhausted without introducing a deeper algorithmic change.
