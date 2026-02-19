use primal::Sieve;
use std::time::Instant;

// ── Meissel-Lehmer prime counting ────────────────────────────────────────────
//
// Extension of Lucy_Hedgehog: sieve primes only up to N^{1/3} (not N^{1/2}),
// then compute the remaining "P₂" contribution analytically.
//
// Key insight: for primes p > N^{1/3}, S_a(N/(j·p)) is frozen after the
// partial sieve — no subsequent prime modifies it. This allows us to compute
// their contribution as a simple sum over stored array values.
//
// Complexity: O(N^{2/3}) time, O(√N) space.

/// Integer square root (exact).
fn isqrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = (n as f64).sqrt() as u64;
    while x > n / x { x -= 1; }
    while (x + 1) <= n / (x + 1) { x += 1; }
    x
}

/// Integer cube root (exact).
fn icbrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = (n as f64).cbrt() as u64 + 1;
    while x > 0 && x as u128 * x as u128 * x as u128 > n as u128 { x -= 1; }
    while (x + 1) as u128 * (x + 1) as u128 * (x + 1) as u128 <= n as u128 { x += 1; }
    x
}

/// Count primes up to n using Meissel-Lehmer with Lucy_Hedgehog partial sieve.
fn count_primes(n: u64) -> u64 {
    if n < 2 { return 0; }
    if n < 3 { return 1; }
    if n < 5 { return 2; }

    let v = isqrt(n) as usize;
    let cbrt_n = icbrt(n) as usize;

    // small[j] = S(j), large[j] = S(⌊N/j⌋)
    let mut small = vec![0i32; v + 1];
    let mut large = vec![0i64; v + 1];

    for j in 1..=v {
        small[j] = j as i32 - 1;
        large[j] = (n / j as u64) as i64 - 1;
    }

    // Reciprocal table for fast division
    let mut recip = vec![0u64; v + 1];
    for j in 1..=v {
        recip[j] = ((1u128 << 64) / j as u128 + 1) as u64;
    }

    // Phase 1: Lucy_Hedgehog sieve for primes up to cbrt(N)
    let prime_sieve = Sieve::new(v);
    let sieve_primes: Vec<usize> = prime_sieve.primes_from(2)
        .take_while(|&p| p <= cbrt_n)
        .collect();
    let all_primes: Vec<usize> = prime_sieve.primes_from(2)
        .take_while(|&p| (p as u64) * (p as u64) <= n)
        .collect();

    for &p in &sieve_primes {
        let pcnt = small[p - 1] as i64;
        let j_end = std::cmp::min(v, (n / (p as u64 * p as u64)) as usize);
        let crossover = v / p;
        let n_div_p = n / p as u64;

        // Branch 1: j*p <= v, use large[j*p]
        let branch1_end = std::cmp::min(crossover, j_end);
        unsafe {
            for j in 1..=branch1_end {
                *large.get_unchecked_mut(j) -= *large.get_unchecked(j * p) - pcnt;
            }
        }

        // Branch 2: two-phase harmonic iteration
        if crossover < j_end {
            let sqrt_x = isqrt(n_div_p) as usize;

            // Phase A: j ≤ √(n/p), reciprocal multiply, 4× unroll
            let phase_a_end = std::cmp::min(sqrt_x, j_end);
            if crossover < phase_a_end {
                unsafe {
                    let mut j = crossover + 1;
                    let end4 = phase_a_end.saturating_sub(3);
                    while j <= end4 {
                        let q0 = ((n_div_p as u128 * *recip.get_unchecked(j) as u128) >> 64) as usize;
                        let q1 = ((n_div_p as u128 * *recip.get_unchecked(j+1) as u128) >> 64) as usize;
                        let q2 = ((n_div_p as u128 * *recip.get_unchecked(j+2) as u128) >> 64) as usize;
                        let q3 = ((n_div_p as u128 * *recip.get_unchecked(j+3) as u128) >> 64) as usize;
                        *large.get_unchecked_mut(j) -= *small.get_unchecked(q0) as i64 - pcnt;
                        *large.get_unchecked_mut(j+1) -= *small.get_unchecked(q1) as i64 - pcnt;
                        *large.get_unchecked_mut(j+2) -= *small.get_unchecked(q2) as i64 - pcnt;
                        *large.get_unchecked_mut(j+3) -= *small.get_unchecked(q3) as i64 - pcnt;
                        j += 4;
                    }
                    while j <= phase_a_end {
                        let q = ((n_div_p as u128 * *recip.get_unchecked(j) as u128) >> 64) as usize;
                        *large.get_unchecked_mut(j) -= *small.get_unchecked(q) as i64 - pcnt;
                        j += 1;
                    }
                }
            }

            // Phase B: j > √(n/p), iterate q downward
            if phase_a_end < j_end {
                let q_start = if phase_a_end >= crossover + 1 {
                    (n_div_p / (phase_a_end + 1) as u64) as usize
                } else {
                    (n_div_p / (crossover + 1) as u64) as usize
                };
                let q_end = (n_div_p / j_end as u64) as usize;
                let mut first_j = phase_a_end + 1;

                if p <= 7 {
                    let mut last_j = std::cmp::min((n_div_p / q_start as u64) as usize, j_end);
                    unsafe {
                        for q in (q_end..=q_start).rev() {
                            if first_j <= last_j {
                                let delta = *small.get_unchecked(q) as i64 - pcnt;
                                for jj in first_j..=last_j {
                                    *large.get_unchecked_mut(jj) -= delta;
                                }
                            }
                            first_j = last_j + 1;
                            if first_j > j_end { break; }
                            if q > q_end {
                                let qm1 = (q - 1) as u64;
                                while (last_j as u64 + 1) * qm1 <= n_div_p {
                                    last_j += 1;
                                }
                                if last_j > j_end { last_j = j_end; }
                            }
                        }
                    }
                } else {
                    unsafe {
                        let mut q = q_start;
                        let end4 = q_end.saturating_add(4);
                        while q >= end4 {
                            let lj0 = ((n_div_p as u128 * *recip.get_unchecked(q) as u128) >> 64) as usize;
                            let lj1 = ((n_div_p as u128 * *recip.get_unchecked(q-1) as u128) >> 64) as usize;
                            let lj2 = ((n_div_p as u128 * *recip.get_unchecked(q-2) as u128) >> 64) as usize;
                            let lj3 = ((n_div_p as u128 * *recip.get_unchecked(q-3) as u128) >> 64) as usize;
                            if first_j <= lj0 {
                                let d = *small.get_unchecked(q) as i64 - pcnt;
                                for jj in first_j..=lj0 { *large.get_unchecked_mut(jj) -= d; }
                                first_j = lj0 + 1;
                            }
                            if first_j <= lj1 {
                                let d = *small.get_unchecked(q-1) as i64 - pcnt;
                                for jj in first_j..=lj1 { *large.get_unchecked_mut(jj) -= d; }
                                first_j = lj1 + 1;
                            }
                            if first_j <= lj2 {
                                let d = *small.get_unchecked(q-2) as i64 - pcnt;
                                for jj in first_j..=lj2 { *large.get_unchecked_mut(jj) -= d; }
                                first_j = lj2 + 1;
                            }
                            if first_j <= lj3 {
                                let d = *small.get_unchecked(q-3) as i64 - pcnt;
                                for jj in first_j..=lj3 { *large.get_unchecked_mut(jj) -= d; }
                                first_j = lj3 + 1;
                            }
                            if first_j > j_end { break; }
                            q -= 4;
                        }
                        while q >= q_end {
                            let last_j = std::cmp::min(
                                ((n_div_p as u128 * *recip.get_unchecked(q) as u128) >> 64) as usize,
                                j_end);
                            if first_j <= last_j {
                                let delta = *small.get_unchecked(q) as i64 - pcnt;
                                for jj in first_j..=last_j {
                                    *large.get_unchecked_mut(jj) -= delta;
                                }
                                first_j = last_j + 1;
                            }
                            if first_j > j_end { break; }
                            q -= 1;
                        }
                    }
                }
            }
        }

        // Update small values (reverse order, 4× unroll)
        if p * p <= v {
            let pcnt32 = pcnt as i32;
            if p == 2 {
                unsafe {
                    let mut j = v;
                    while j >= 7 {
                        *small.get_unchecked_mut(j) -= *small.get_unchecked(j >> 1) - pcnt32;
                        *small.get_unchecked_mut(j-1) -= *small.get_unchecked((j-1) >> 1) - pcnt32;
                        *small.get_unchecked_mut(j-2) -= *small.get_unchecked((j-2) >> 1) - pcnt32;
                        *small.get_unchecked_mut(j-3) -= *small.get_unchecked((j-3) >> 1) - pcnt32;
                        j -= 4;
                    }
                    while j >= 4 {
                        *small.get_unchecked_mut(j) -= *small.get_unchecked(j >> 1) - pcnt32;
                        j -= 1;
                    }
                }
            } else {
                let recip_p = ((1u64 << 40) + p as u64 - 1) / p as u64;
                unsafe {
                    let pp = p * p;
                    let mut j = v;
                    let end4 = pp + 3;
                    while j >= end4 {
                        let q0 = ((j as u64 * recip_p) >> 40) as usize;
                        let q1 = (((j-1) as u64 * recip_p) >> 40) as usize;
                        let q2 = (((j-2) as u64 * recip_p) >> 40) as usize;
                        let q3 = (((j-3) as u64 * recip_p) >> 40) as usize;
                        *small.get_unchecked_mut(j) -= *small.get_unchecked(q0) - pcnt32;
                        *small.get_unchecked_mut(j-1) -= *small.get_unchecked(q1) - pcnt32;
                        *small.get_unchecked_mut(j-2) -= *small.get_unchecked(q2) - pcnt32;
                        *small.get_unchecked_mut(j-3) -= *small.get_unchecked(q3) - pcnt32;
                        j -= 4;
                    }
                    while j >= pp {
                        let q = ((j as u64 * recip_p) >> 40) as usize;
                        *small.get_unchecked_mut(j) -= *small.get_unchecked(q) - pcnt32;
                        j -= 1;
                    }
                }
            }
        }
    }

    // Phase 2: P₂ contribution from primes p with cbrt(N) < p ≤ √N
    //
    // For each such prime p, the full Lucy_Hedgehog would do:
    //   large[1] -= large[p] - pcnt    (Branch 1 at j=1)
    // plus updates to large[j>1] for j up to n/p².
    //
    // Key insight: the j>1 updates DON'T propagate to large[1] because
    // subsequent primes' Branch 1 reads large[p'] where p' > j_end(p),
    // so the modified entries are never read. Therefore P₂ needs only j=1:
    //
    //   P₂ = Σ [S_a(n/p) - π(p-1)] = Σ [large[p] - (k-1)]
    //
    // where S_a values are frozen (proven: p > N^{1/3} → no prime between
    // p_a and p modifies large[p], since j_end < p for all such primes).
    let mut p2: i64 = 0;

    for (idx, &p) in all_primes.iter().enumerate() {
        if p <= cbrt_n { continue; }
        let pcnt = idx as i64; // π(p-1) = idx (0-based)
        unsafe {
            p2 += *large.get_unchecked(p) - pcnt;
        }
    }

    (large[1] - p2) as u64
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V3 — Meissel-Lehmer Method                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    struct Case { limit: u64, label: &'static str, expected: u64 }

    let cases = [
        Case { limit:              1_000, label: "1 Thousand",   expected:             168 },
        Case { limit:          1_000_000, label: "1 Million",    expected:          78_498 },
        Case { limit:      1_000_000_000, label: "1 Billion",    expected:      50_847_534 },
        Case { limit:     10_000_000_000, label: "10 Billion",   expected:     455_052_511 },
        Case { limit:    100_000_000_000, label: "100 Billion",  expected:   4_118_054_813 },
        Case { limit:  1_000_000_000_000, label: "1 Trillion",   expected: 37_607_912_018 },
        Case { limit: 10_000_000_000_000, label: "10 Trillion",  expected: 346_065_536_839 },
    ];

    println!("{:<15} {:>12} {:>18}  {}", "Range", "Time", "Primes Found", "Status");
    println!("{}", "─".repeat(65));

    for c in &cases {
        let t0 = Instant::now();
        let count = count_primes(c.limit);
        let secs = t0.elapsed().as_secs_f64();
        let check = if count == c.expected { "✓" } else { "✗ MISMATCH" };

        println!(
            "{:<15} {:>10.5}s   {:>16}  {}  (expected: {})",
            c.label, secs, count, check, c.expected
        );
    }
}
