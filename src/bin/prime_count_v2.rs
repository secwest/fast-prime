use std::time::Instant;

// ── Lucy_Hedgehog / Meissel-Lehmer prime counting ──────────────────────────
//
// Computes π(N) exactly without sieving all numbers up to N.
// Complexity: O(N^{3/4} / ln N) time, O(√N) space.
//
// Key insight: we only need π(v) for v in the set {1..√N} ∪ {⌊N/k⌋ : k=1..√N}.
// This set has O(√N) elements. We iteratively sieve these values.

/// Integer square root (exact).
fn isqrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = (n as f64).sqrt() as u64;
    while x > n / x { x -= 1; }
    while (x + 1) <= n / (x + 1) { x += 1; }
    x
}

/// Count primes up to n using the Lucy_Hedgehog combinatorial method.
fn count_primes(n: u64) -> u64 {
    if n < 2 { return 0; }
    if n < 3 { return 1; }
    if n < 5 { return 2; }

    let v = isqrt(n) as usize;

    // small[j] = S(j) = count of integers in [2,j] surviving sieve by primes so far (≤ √N, fits i32)
    // large[j] = S(⌊N/j⌋) = same for ⌊N/j⌋ (can exceed 2^31 for n > ~4B, needs i64)
    let mut small = vec![0i32; v + 1];
    let mut large = vec![0i64; v + 1];

    // Initialize with p=2 already sieved: S(x, 2) = x - floor(x/2) for x >= 2
    // This counts odd numbers in [1,x] (including 1 for the prime 2 itself)
    for j in 1..=v {
        small[j] = (j as i32 - 1) - (j as i32 / 2 - 1).max(0); // j - 1 - (j/2 - 1) = j - j/2
        large[j] = (n / j as u64) as i64 - 1 - ((n / (2 * j as u64)) as i64 - 1).max(0);
    }

    // Sieve: iterate only over odd primes p = 3, 5, 7, ...
    let mut p = 3;
    while p <= v {
        if small[p] <= small[p - 1] { p += 2; continue; } // p is composite

        let pcnt = small[p - 1] as i64;
        let p2 = p as u64 * p as u64;
        if p2 > n { break; }

        let j_end = std::cmp::min(v, (n / p2) as usize);
        let crossover = v / p;
        let n_div_p = n / p as u64;

        // Branch 1: j*p <= v, use large[j*p]
        let branch1_end = std::cmp::min(crossover, j_end);
        unsafe {
            for j in 1..=branch1_end {
                *large.get_unchecked_mut(j) -= *large.get_unchecked(j * p) - pcnt;
            }
        }

        // Branch 2: harmonic block technique
        if crossover < j_end {
            let mut j = crossover + 1;
            while j <= j_end {
                let q = (n_div_p / j as u64) as usize;
                let last_j = std::cmp::min((n_div_p / q as u64) as usize, j_end);
                let delta = unsafe { *small.get_unchecked(q) } as i64 - pcnt;
                unsafe {
                    for jj in j..=last_j {
                        *large.get_unchecked_mut(jj) -= delta;
                    }
                }
                j = last_j + 1;
            }
        }

        // Update small values (reverse order)
        if p * p <= v {
            let pcnt32 = pcnt as i32;
            let recip_p = ((1u64 << 40) + p as u64 - 1) / p as u64;
            unsafe {
                for j in (p * p..=v).rev() {
                    let q = ((j as u64 * recip_p) >> 40) as usize;
                    *small.get_unchecked_mut(j) -= *small.get_unchecked(q) - pcnt32;
                }
            }
        }
        p += 2;
    }

    large[1] as u64
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V2 — Lucy_Hedgehog Combinatorial Method     ║");
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
