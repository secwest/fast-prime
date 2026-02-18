# fast-prime

A highly optimized prime counting sieve in Rust, targeting modern hybrid-core CPUs.

Counts all primes up to N using a segmented Sieve of Eratosthenes with wheel mod 30 factorization and parallel execution via [Rayon](https://github.com/rayon-rs/rayon).

## Benchmarks — Intel Core Ultra 9 285K (24 threads)

```
┌─────────────┬──────────────┬──────────────────┐
│ Range       │ Time         │ Primes Found     │
├─────────────┼──────────────┼──────────────────┤
│ 1 Thousand  │    0.00002s  │              168 │
│ 1 Million   │    0.00007s  │           78,498 │
│ 1 Billion   │    0.01311s  │       50,847,534 │
│ 10 Billion  │    0.14593s  │      455,052,511 │
│ 100 Billion │    1.59081s  │    4,118,054,813 │
│ 1 Trillion  │   19.26516s  │   37,607,912,018 │
└─────────────┴──────────────┴──────────────────┘
```

## Key Optimizations

- **Wheel mod 30** — Only sieves 8 residues per 30 numbers (those coprime to 2, 3, 5), reducing candidate count by ~73% vs odds-only
- **1 byte = 30 numbers** — Each byte encodes 8 wheel residues as individual bits, providing excellent cache utilization
- **Adaptive segment sizing** — Segments scale between 16KB–512KB to ensure enough parallel work units for effective work-stealing across heterogeneous P-cores and E-cores
- **Pre-sieve pattern** — Composites of primes 7, 11, 13 are pre-computed in a 1001-byte repeating pattern and tiled via memcpy, eliminating inner-loop work for the three most frequent small primes
- **Precomputed wheel tables** — `TARGET_K_MOD` lookup table eliminates per-residue modular inverse computation; common subexpressions hoisted out of the per-residue loop
- **Unrolled inner loop** — 4× unrolled sieve marking for small primes that hit many times per segment
- **Thread-local sieve buffers** — Reused via `map_init` to avoid repeated allocation
- **`target-cpu=native`** — Compiled with native CPU instructions via `.cargo/config.toml`
- **Rayon work-stealing** — Naturally balances load across fast P-cores and slower E-cores without explicit affinity pinning

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
2. **Segment** — Divide (√N, N] into cache-friendly segments, each covering 30 × segment_bytes numbers
3. **Parallel sieve** — Rayon dispatches segments across all cores; each worker marks composites for all sieving primes using wheel-30 residue classes stepping by p bytes
4. **Count** — Survivors (zero bits) counted with hardware `POPCNT` via `count_ones()`, 64 bits at a time
5. **Sum** — Per-segment counts reduced in parallel

## Dependencies

- [primal](https://crates.io/crates/primal) — Bootstrap sieve for small primes ≤ √N
- [rayon](https://crates.io/crates/rayon) — Data-parallel work-stealing thread pool

## License

MIT
