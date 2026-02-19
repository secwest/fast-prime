use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;

// ── Lagarias-Miller-Odlyzko prime counting ───────────────────────────────────
//
// Formula: π(x) = S1(x,a) + S2(x,a) + π(y) - 1 - P2(x,a)
// where y = x^{1/3}, a = π(y), c = min(a, 6), and:
//   S1  = "ordinary leaves" — Σ μ(n)·φ(x/n, c)   for squarefree n ≤ y, lpf(n) > p_c
//   S2  = "special leaves"  — -Σ μ(m)·φ(x/(p_b·m), b-1)
//   P2  = contribution from pairs of large primes
//
// Reference: Kim Walisch, primecount (pi_lmo1.cpp, pi_lmo_parallel.cpp)

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

// ── Precomputation tables ────────────────────────────────────────────────────

/// Generate least prime factor table for [0, limit].
fn generate_lpf(limit: usize) -> Vec<i32> {
    let mut lpf = vec![i32::MAX; limit + 1];
    lpf[0] = 0;
    for p in 2..=limit {
        if lpf[p] != i32::MAX { continue; }
        for m in (p..=limit).step_by(p) {
            if lpf[m] == i32::MAX {
                lpf[m] = p as i32;
            }
        }
    }
    lpf
}

/// Generate Möbius function table for [0, limit].
fn generate_mu(limit: usize) -> Vec<i8> {
    let mut mu = vec![1i8; limit + 1];
    mu[0] = 0;
    let mut is_prime = vec![true; limit + 1];
    for p in 2..=limit {
        if !is_prime[p] { continue; }
        for m in (p..=limit).step_by(p) {
            if m > p { is_prime[m] = false; }
            mu[m] = -mu[m];
        }
        let p2 = p * p;
        for m in (p2..=limit).step_by(p2) {
            mu[m] = 0;
        }
    }
    mu
}

// ── PhiTiny: φ(x, c) for small c using precomputed wheel ────────────────────

const TINY_PRIMES: [u64; 7] = [0, 2, 3, 5, 7, 11, 13];

struct PhiTinyCache {
    pc: u64,
    phi_pc: u64,
    partial: Vec<u64>,
}

impl PhiTinyCache {
    fn new(c: usize) -> Self {
        let mut pc = 1u64;
        let mut phi_pc = 1u64;
        for i in 1..=c { pc *= TINY_PRIMES[i]; phi_pc *= TINY_PRIMES[i] - 1; }
        let mut partial = vec![0u64; pc as usize + 1];
        for k in 1..=pc as usize {
            partial[k] = partial[k - 1];
            let kk = k as u64;
            let mut coprime = true;
            for i in 1..=c {
                if kk % TINY_PRIMES[i] == 0 { coprime = false; break; }
            }
            if coprime { partial[k] += 1; }
        }
        PhiTinyCache { pc, phi_pc, partial }
    }

    fn phi(&self, x: u64) -> i64 {
        if x == 0 { return 0; }
        let full = (x / self.pc) * self.phi_pc;
        let rem = (x % self.pc) as usize;
        (full + self.partial[rem]) as i64
    }
}

// ── P2: Pairs of large primes ────────────────────────────────────────────────

fn compute_p2(x: u64, y: usize, pi_y: usize) -> i64 {
    let sqrt_x = isqrt(x) as usize;
    if y >= sqrt_x { return 0; }

    let max_val = (x / (y as u64 + 1)) as usize;
    let p2_sieve = Sieve::new(std::cmp::max(max_val, sqrt_x));

    let pi_sqrt_x = p2_sieve.prime_pi(sqrt_x);

    // Collect primes in (y, sqrt_x]
    let p2_primes: Vec<usize> = p2_sieve.primes_from(y + 1)
        .take_while(|&p| p <= sqrt_x)
        .collect();

    // Parallel sum of π(x/p) for each prime p
    let p2: i64 = p2_primes.par_iter()
        .map(|&p| p2_sieve.prime_pi((x / p as u64) as usize) as i64)
        .sum();

    let choose2 = |n: i64| n * (n - 1) / 2;
    p2 - choose2(pi_sqrt_x as i64) + choose2(pi_y as i64)
}

// ── S2: Special leaves via segmented sieve with POPCNT ───────────────────────

/// Bit-packed sieve segment.
struct BitSieve {
    bits: Vec<u64>,
    len: usize,
    total: i64,
}

impl BitSieve {
    fn new(max_len: usize) -> Self {
        BitSieve { bits: vec![0u64; (max_len + 63) / 64], len: 0, total: 0 }
    }

    /// Count set bits in positions [0, pos].
    #[inline]
    fn count(&self, pos: usize) -> i64 {
        let full = pos / 64;
        let bit = pos % 64;
        let mut cnt = 0u64;
        for i in 0..full {
            cnt += unsafe { *self.bits.get_unchecked(i) }.count_ones() as u64;
        }
        let mask = (2u64 << bit) - 1;
        cnt += (unsafe { *self.bits.get_unchecked(full) } & mask).count_ones() as u64;
        cnt as i64
    }

    /// Count set bits in (prev_pos, pos] given that count(prev_pos) is known.
    #[inline]
    fn count_delta(&self, prev_pos: usize, pos: usize) -> i64 {
        let w0 = prev_pos / 64;
        let b0 = prev_pos % 64;
        let w1 = pos / 64;
        let b1 = pos % 64;
        if w0 == w1 {
            let word = unsafe { *self.bits.get_unchecked(w0) };
            let hi_mask = (2u64 << b1) - 1;
            let lo_mask = (2u64 << b0) - 1;
            return (word & (hi_mask ^ lo_mask)).count_ones() as i64;
        }
        let mut cnt = 0u64;
        // Bits above b0 in word w0
        let first_mask = if b0 < 63 { u64::MAX << (b0 + 1) } else { 0 };
        cnt += (unsafe { *self.bits.get_unchecked(w0) } & first_mask).count_ones() as u64;
        // Full words in between
        for i in (w0 + 1)..w1 {
            cnt += unsafe { *self.bits.get_unchecked(i) }.count_ones() as u64;
        }
        // Partial word w1 up to b1
        let mask = (2u64 << b1) - 1;
        cnt += (unsafe { *self.bits.get_unchecked(w1) } & mask).count_ones() as u64;
        cnt as i64
    }

    fn count_total(&self) -> i64 {
        self.total
    }

    #[inline]
    fn cross_off(&mut self, pos: usize) {
        let w = pos / 64;
        let b = pos % 64;
        let old = unsafe { *self.bits.get_unchecked(w) };
        let was_set = ((old >> b) & 1) as i64;
        unsafe { *self.bits.get_unchecked_mut(w) = old & !(1u64 << b); }
        self.total -= was_set;
    }
}

/// Pre-sieve template for first c primes. Period = lcm(p_1..p_c).
struct PreSieveTemplate {
    // Template stored as 2× period bits to avoid wrapping edge cases
    bits: Vec<u64>,
    period: usize,
}

impl PreSieveTemplate {
    fn new(primes: &[u32], c: usize) -> Self {
        let mut period: usize = 1;
        for b in 1..=c {
            period *= primes[b] as usize;
        }
        // Build one period of the template
        let mut tpl = vec![true; period];
        for b in 1..=c {
            let p = primes[b] as usize;
            let mut k = 0;
            while k < period {
                tpl[k] = false;
                k += p;
            }
        }
        // Store 2 periods as packed u64 words
        let double_period = period * 2;
        let nwords = (double_period + 63) / 64;
        let mut bits = vec![0u64; nwords];
        for i in 0..double_period {
            if tpl[i % period] {
                bits[i / 64] |= 1u64 << (i % 64);
            }
        }
        PreSieveTemplate { bits, period }
    }

    /// Get 64 aligned template bits starting at template position `start`.
    /// Caller must ensure start < period.
    #[inline]
    fn get_word(&self, start: usize) -> u64 {
        let w = start / 64;
        let bit_off = start % 64;
        if bit_off == 0 {
            unsafe { *self.bits.get_unchecked(w) }
        } else {
            let lo = unsafe { *self.bits.get_unchecked(w) } >> bit_off;
            let hi = unsafe { *self.bits.get_unchecked(w + 1) } << (64 - bit_off);
            lo | hi
        }
    }

    /// Apply template to sieve segment starting at `low`, combining with reset.
    /// This replaces separate reset() + apply() calls.
    fn init_sieve(&self, sieve: &mut BitSieve, low: usize, len: usize) {
        sieve.len = len;
        let nwords = (len + 63) / 64;
        let mut tpl_pos = low % self.period;
        let mut total = 0i64;
        for w in 0..nwords {
            let word = self.get_word(tpl_pos);
            unsafe { *sieve.bits.get_unchecked_mut(w) = word; }
            total += word.count_ones() as i64;
            tpl_pos += 64;
            if tpl_pos >= self.period { tpl_pos -= self.period; }
        }
        // Clear excess bits in the last word
        let excess = nwords * 64 - len;
        if excess > 0 {
            let last = unsafe { *sieve.bits.get_unchecked(nwords - 1) };
            let masked = last & (u64::MAX >> excess);
            total -= (last.count_ones() as i64) - (masked.count_ones() as i64);
            unsafe { *sieve.bits.get_unchecked_mut(nwords - 1) = masked; }
        }
        // Clear position 0 in first segment (integer 0 not in [1,x])
        if low == 0 {
            let old = unsafe { *sieve.bits.get_unchecked(0) };
            let was_set = (old & 1) as i64;
            unsafe { *sieve.bits.get_unchecked_mut(0) = old & !1; }
            total -= was_set;
        }
        sieve.total = total;
    }
}

/// Compute S2 using segmented sieve (matches pi_lmo_parallel structure).
fn compute_s2(x: u64, y: usize, c: usize, primes: &[u32],
              lpf: &[i32], mu: &[i8], pi: &[u32]) -> i64 {
    let z = (x / y as u64) as usize;
    let sqrt_y = isqrt(y as u64) as usize;
    let pi_sqrty = pi[std::cmp::min(sqrt_y, y)] as usize;
    let pi_y = pi[y] as usize;

    // Segment size: larger segments reduce per-segment overhead (init, start calc)
    // 1<<17 = 128K bits = 16KB, fits comfortably in L1 cache (48KB)
    let segment_size = std::cmp::max(isqrt(z as u64) as usize, 1 << 17).next_power_of_two();

    // Pre-sieve template for first c primes
    let template = PreSieveTemplate::new(primes, std::cmp::min(c, primes.len() - 1));

    // phi[b] accumulates φ(low, b-1) across segments
    let mut phi: Vec<i64> = vec![0i64; primes.len()];
    let mut next: Vec<usize> = (0..primes.len()).map(|b| primes[b] as usize).collect();

    let mut sieve = BitSieve::new(segment_size);
    let mut s2: i64 = 0;
    let mut low: usize = 0;

    while low <= z {
        let high = std::cmp::min(low + segment_size, z + 1);
        let seg_len = high - low;
        let low1 = std::cmp::max(low, 1);

        // Combined reset + template apply + position 0 handling
        template.init_sieve(&mut sieve, low, seg_len);

        // Determine which b values have special leaves in this segment
        let max_b = if low1 > 0 {
            pi[std::cmp::min(isqrt(x / low1 as u64) as usize, y)] as usize
        } else { pi_y };
        let max_b = std::cmp::min(max_b, pi_y - 1);

        let mut b = c + 1;

        // Hard special leaves: c+1 ≤ b ≤ π(√y)
        while b <= std::cmp::min(pi_sqrty, max_b) && b < primes.len() {
            let prime = primes[b] as u64;
            let min_m = std::cmp::max(
                x / (prime * high as u64),
                y as u64 / prime
            ) as usize;
            let max_m = std::cmp::min(
                x / (prime * low1 as u64),
                y as u64
            ) as usize;

            if prime as usize >= max_m { break; }

            // Iterate m descending (xpm ascending) with incremental count
            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            for m in (min_m + 1..=max_m).rev() {
                if mu[m] != 0 && (prime as i32) < lpf[m] {
                    let xpm = (x / (prime * m as u64)) as usize;
                    if xpm >= low && xpm < high {
                        let pos = xpm - low;
                        let count = match prev_pos {
                            None => {
                                running_count = sieve.count(pos);
                                running_count
                            }
                            Some(pp) => {
                                running_count += sieve.count_delta(pp, pos);
                                running_count
                            }
                        };
                        s2 -= mu[m] as i64 * (phi[b] + count);
                        prev_pos = Some(pos);
                    }
                }
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            let start = if next[b] >= low { next[b] - low } else {
                let rem = (low - next[b]) % p;
                if rem == 0 { 0 } else { p - rem }
            };
            let mut k = start;
            while k < seg_len {
                sieve.cross_off(k);
                k += p;
            }
            next[b] = low + k;
            b += 1;
        }

        // Easy special leaves: π(√y) < b ≤ max_b (two-prime products)
        while b <= max_b && b < primes.len() {
            let prime = primes[b] as u64;
            let l_max = std::cmp::min(
                (x / (prime * low1 as u64)) as usize,
                y
            );
            let mut l = pi[std::cmp::min(l_max, y)] as usize;
            let min_m = std::cmp::max(
                (x / (prime * high as u64)) as usize,
                prime as usize
            );

            if l < primes.len() && prime as usize >= primes[l] as usize { break; }

            // q = primes[l] decreases => xpq increases => positions are ascending
            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            while l > 0 && l < primes.len() && (primes[l] as usize) > min_m {
                let xpq = (x / (prime * primes[l] as u64)) as usize;
                if xpq >= low && xpq < high {
                    let pos = xpq - low;
                    let count = match prev_pos {
                        None => {
                            running_count = sieve.count(pos);
                            running_count
                        }
                        Some(pp) if pos > pp => {
                            running_count += sieve.count_delta(pp, pos);
                            running_count
                        }
                        _ => {
                            // Same or earlier position — recalculate
                            running_count = sieve.count(pos);
                            running_count
                        }
                    };
                    s2 += phi[b] + count;
                    prev_pos = Some(pos);
                }
                l -= 1;
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            let start = if next[b] >= low { next[b] - low } else {
                let rem = (low - next[b]) % p;
                if rem == 0 { 0 } else { p - rem }
            };
            let mut k = start;
            while k < seg_len {
                sieve.cross_off(k);
                k += p;
            }
            next[b] = low + k;
            b += 1;
        }

        low += segment_size;
    }

    s2
}

// ── Main counting function ───────────────────────────────────────────────────

/// Generate π(n) lookup table for n in [0, limit].
fn generate_pi(limit: usize, sieve: &Sieve) -> Vec<u32> {
    let mut pi = vec![0u32; limit + 1];
    let mut count = 0u32;
    for n in 0..=limit {
        if n >= 2 && sieve.is_prime(n) {
            count += 1;
        }
        pi[n] = count;
    }
    pi
}

fn count_primes(x: u64) -> u64 {
    if x < 2 { return 0; }

    let alpha = 2.0;
    let y = std::cmp::max((icbrt(x) as f64 * alpha) as usize, 1);

    // For small x, use primal directly
    if x <= 10_000 {
        return Sieve::new(x as usize).prime_pi(x as usize) as u64;
    }

    let prime_sieve = Sieve::new(y);
    let mut primes: Vec<u32> = vec![0];
    primes.extend(prime_sieve.primes_from(2).take_while(|&p| p <= y).map(|p| p as u32));

    let pi_y = primes.len() - 1;
    let c = std::cmp::min(6, pi_y);
    let phi_cache = PhiTinyCache::new(c);
    let lpf = generate_lpf(y);
    let mu = generate_mu(y);
    let pi = generate_pi(y, &prime_sieve);

    // S1: ordinary leaves
    let pc = primes[c];
    let mut s1: i64 = 0;
    for n in 1..=y {
        if mu[n] != 0 && lpf[n] > pc as i32 {
            s1 += mu[n] as i64 * phi_cache.phi(x / n as u64);
        }
    }

    // Run S2 and P2 concurrently: P2 sieve construction overlaps with S2 computation
    let (s2, p2) = std::thread::scope(|s| {
        let p2_handle = s.spawn(|| compute_p2(x, y, pi_y));
        let s2 = compute_s2(x, y, c, &primes, &lpf, &mu, &pi);
        let p2 = p2_handle.join().unwrap();
        (s2, p2)
    });

    let result = s1 + s2 + pi_y as i64 - 1 - p2;
    result as u64
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V4 — Lagarias-Miller-Odlyzko (LMO)         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    struct Case { limit: u64, label: &'static str, expected: u64 }

    let cases = [
        Case { limit:              1_000, label: "1 Thousand",   expected:             168 },
        Case { limit:             10_000, label: "10 Thousand",  expected:           1_229 },
        Case { limit:            100_000, label: "100 Thousand", expected:           9_592 },
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
