# fast-prime

A highly optimized prime counting sieve in Rust, targeting modern hybrid-core CPUs.

Counts all primes up to N using a segmented Sieve of Eratosthenes with wheel mod 30 factorization, two-level cache-aware segmentation, and parallel execution via [Rayon](https://github.com/rayon-rs/rayon).

## Benchmarks — Intel Core Ultra 9 285K (24 threads)

```
┌─────────────┬──────────────┬──────────────────┐
│ Range       │ Time         │ Primes Found     │
├─────────────┼──────────────┼──────────────────┤
│ 1 Thousand  │    0.00004s  │              168 │
│ 1 Million   │    0.00009s  │           78,498 │
│ 1 Billion   │    0.00600s  │       50,847,534 │
│ 10 Billion  │    0.06500s  │      455,052,511 │
│ 100 Billion │    0.71300s  │    4,118,054,813 │
│ 1 Trillion  │    8.60000s  │   37,607,912,018 │
└─────────────┴──────────────┴──────────────────┘
```

### Comparison vs Reference Implementations

| Range | This (Ultra 9 285K) | Strix Halo Reference | Speedup |
|---|---|---|---|
| 1 Billion | 0.006s | 0.011s | **1.83×** |
| 10 Billion | 0.065s | 0.109s | **1.68×** |
| 100 Billion | 0.713s | 1.483s | **2.08×** |
| 1 Trillion | 8.600s | 25.820s | **3.00×** |

## Key Optimizations

See [OPTIMIZATIONS.md](OPTIMIZATIONS.md) for a detailed log of every optimization tried, including results (positive and negative).

### Architecture

- **Wheel mod 30** — Only sieves 8 residues per 30 numbers (coprime to 2, 3, 5), reducing candidate count by ~73% vs odds-only. Each byte encodes 8 wheel residues as individual bits.
- **Two-level cache-aware segmentation** — L2 segments (1MB) for parallelism, with L1 sub-segments (24KB) for tiny primes. This is the single biggest optimization, providing ~2× speedup.
- **Extended pre-sieve pattern** — Composites of primes 7, 11, 13, 17, 19 are pre-computed in a 323KB repeating pattern and tiled via memcpy, eliminating inner-loop work for five frequent small primes.
- **4-tier prime classification** — Tiny primes (L1 sub-segmented with carry-forward), small primes (4× unrolled), medium primes (simple loop), large primes (single-write). Each tier uses the optimal marking strategy.
- **Carry-forward L1 sub-segment starts** — Compute starts once per L2 segment for tiny primes, carry forward across L1 sub-segments (eliminates ~97% of compute_starts calls for tiny primes).
- **DK_TABLE precomputation** — Const lookup table for delta-k values eliminates branch+compare in start computation.
- **Barrett fast division** — Precomputed reciprocals replace costly u64 division in `compute_starts` with 128-bit multiply.
- **Precomputed wheel tables** — `TARGET_K_MOD` lookup table eliminates per-residue modular inverse computation.
- **Lazy presieve (OnceLock)** — 323KB pattern built once, reused across all calls.
- **Adaptive parallel granularity** — Segments scale between 8KB–1MB to ensure enough work units for work-stealing across heterogeneous P-cores and E-cores.
- **`target-cpu=native`** — Compiled with native CPU instructions (AVX2, POPCNT, BMI2).
- **Rayon work-stealing** — Naturally balances load across fast P-cores and slower E-cores without explicit affinity pinning.

## Building

Requires [Rust](https://rustup.rs/) (1.70+).

```sh
cargo build --release
./target/release/prime-count
```

The build is configured with aggressive optimizations in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
```

And native CPU targeting in `.cargo/config.toml`:

```toml
[build]
rustflags = ["-C", "target-cpu=native"]
```

## Algorithm

1. **Bootstrap** — Build a small sieve up to √N using [primal](https://crates.io/crates/primal) to collect sieving primes
2. **Classify primes** — Split sieving primes into 4 tiers (tiny/small/medium/large) with precomputed Barrett reciprocals
3. **Segment** — Divide (√N, N] into 1MB L2-cache-friendly segments
4. **Parallel sieve** — Rayon dispatches segments across all cores. For each segment:
   - Pre-sieve pattern tiled from 323KB template (primes 7, 11, 13, 17, 19)
   - Tiny primes processed in 24KB L1 sub-segments for cache locality
   - Small/medium primes sieved across full L2 segment
   - Large primes: single byte write per residue
5. **Count** — Survivors (zero bits) counted with hardware `POPCNT` via `count_ones()`, 64 bits at a time
6. **Sum** — Per-segment counts reduced in parallel

## Dependencies

- [primal](https://crates.io/crates/primal) — Bootstrap sieve for small primes ≤ √N
- [rayon](https://crates.io/crates/rayon) — Data-parallel work-stealing thread pool

## License

MIT
