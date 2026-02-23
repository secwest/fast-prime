use mimalloc::MiMalloc;
use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

// ── Primesieve FFI bindings ──────────────────────────────────────────────────
#[repr(C)]
struct PrimesieveIterator {
    i: usize,
    size: usize,
    start: u64,
    stop_hint: u64,
    primes: *mut u64,
    memory: *mut std::ffi::c_void,
    is_error: i32,
}

extern "C" {
    fn primesieve_init(it: *mut PrimesieveIterator);
    fn primesieve_free_iterator(it: *mut PrimesieveIterator);
    fn primesieve_jump_to(it: *mut PrimesieveIterator, start: u64, stop_hint: u64);
    fn primesieve_generate_next_primes(it: *mut PrimesieveIterator);
    fn primesieve_set_num_threads(num_threads: i32);
    fn primesieve_count_primes(start: u64, stop: u64) -> u64;
}

impl PrimesieveIterator {
    fn new() -> Self {
        let mut it = PrimesieveIterator {
            i: 0, size: 0, start: 0, stop_hint: 0,
            primes: std::ptr::null_mut(),
            memory: std::ptr::null_mut(),
            is_error: 0,
        };
        unsafe { primesieve_init(&mut it); }
        it
    }

    fn jump_to(&mut self, start: u64, stop_hint: u64) {
        unsafe { primesieve_jump_to(self, start, stop_hint); }
    }

    #[inline]
    fn next_prime(&mut self) -> u64 {
        self.i += 1;
        if self.i >= self.size {
            unsafe { primesieve_generate_next_primes(self); }
        }
        unsafe { *self.primes.add(self.i) }
    }
}

impl Drop for PrimesieveIterator {
    fn drop(&mut self) {
        unsafe { primesieve_free_iterator(self); }
    }
}

// ── Gourdon's Algorithm ──────────────────────────────────────────────────────
//
// Formula: π(x) = A - B + C + D + Φ₀ + Σ
//
// Parameters:
//   y = x^{1/3} · α_y,  with x^{1/3} < y < x^{1/2}
//   z = y · α_z,         with y ≤ z < x^{1/2}
//   k = PhiTiny depth (6 primes: 2,3,5,7,11,13)
//   x* = max(x^{1/4}, ⌈x/y²⌉), clamped to [1, min(y, √(x/y))]
//
// Key advantage over Deleglise-Rivat:
//   Two alpha parameters allow independent tuning of y and z.
//   D (hard leaves) processes fewer leaves than S2_hard by using x* bounds.

fn isqrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = (n as f64).sqrt() as u64;
    while x > n / x { x -= 1; }
    while (x + 1) <= n / (x + 1) { x += 1; }
    x
}

fn icbrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = (n as f64).cbrt() as u64 + 1;
    while x > 0 && x as u128 * x as u128 * x as u128 > n as u128 { x -= 1; }
    while (x + 1) as u128 * (x + 1) as u128 * (x + 1) as u128 <= n as u128 { x += 1; }
    x
}

fn iroot4(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = ((n as f64).sqrt().sqrt()) as u64 + 1;
    while x > 0 && (x as u128).pow(4) > n as u128 { x -= 1; }
    while ((x + 1) as u128).pow(4) <= n as u128 { x += 1; }
    x
}

// ── Precomputation tables ────────────────────────────────────────────────────

fn generate_tables(limit: usize, y: usize) -> (Vec<i8>, Vec<i32>, Vec<bool>) {
    let mut mu = vec![1i8; limit + 1];
    let mut lpf = vec![i32::MAX; limit + 1];
    let mut y_smooth = vec![true; limit + 1];
    mu[0] = 0;
    lpf[0] = 0;
    y_smooth[0] = false;

    let mut is_prime = vec![true; limit + 1];
    if limit >= 1 { is_prime[0] = false; is_prime[1] = false; }

    for p in 2..=limit {
        if !is_prime[p] { continue; }
        if lpf[p] == i32::MAX { lpf[p] = p as i32; }
        mu[p] = -mu[p]; // prime p: mu(p) = -1
        for m in (2 * p..=limit).step_by(p) {
            is_prime[m] = false;
            if lpf[m] == i32::MAX { lpf[m] = p as i32; }
            mu[m] = -mu[m];
        }
        let p2 = p * p;
        if p2 <= limit {
            for m in (p2..=limit).step_by(p2) { mu[m] = 0; }
        }
        // Mark numbers with prime factors > y as not y-smooth
        if p > y {
            for m in (p..=limit).step_by(p) {
                y_smooth[m] = false;
            }
        }
    }

    (mu, lpf, y_smooth)
}

fn generate_pi(limit: usize, sieve: &Sieve) -> Vec<u32> {
    let mut pi = vec![0u32; limit + 1];
    let mut count = 0u32;
    for n in 0..=limit {
        if n >= 2 && sieve.is_prime(n) { count += 1; }
        pi[n] = count;
    }
    pi
}

// ── PhiTiny ──────────────────────────────────────────────────────────────────

const TINY_PRIMES: [u64; 8] = [0, 2, 3, 5, 7, 11, 13, 17];

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

// ── Alpha parameters ─────────────────────────────────────────────────────────

fn get_alpha_gourdon(x: u64) -> (f64, f64) {
    // Allow override via environment
    if let (Ok(ay), Ok(az)) = (std::env::var("ALPHA_Y"), std::env::var("ALPHA_Z")) {
        if let (Ok(ay), Ok(az)) = (ay.parse::<f64>(), az.parse::<f64>()) {
            if ay >= 1.0 && az > 0.0 { return (ay, az); }
        }
    }

    let logx = (x as f64).ln();

    // Lookup table tuned for Intel Core Ultra 9 285K (24 threads, 36MB L3)
    // Opt 28: re-tuned alphas for 3x thread oversubscription — higher ay, lower az at large scales
    // (logx, alpha_y, alpha_z)
    const TABLE: &[(f64, f64, f64)] = &[
        (20.0,  2.0, 1.5),   // x ~ 5e8
        (23.0,  3.0, 1.5),   // x ~ 1e10
        (25.3,  4.0, 2.0),   // x ~ 1e11
        (30.0,  6.0, 2.0),   // x ~ 1e13
        (32.2,  6.0, 2.0),   // x ~ 1e14
        (34.5,  6.0, 2.0),   // x ~ 1e15
        (36.8,  7.0, 1.5),   // x ~ 1e16
        (39.1,  8.0, 1.5),   // x ~ 1e17
        (41.4, 12.0, 1.5),   // x ~ 1e18
        (43.6, 13.0, 1.5),   // x ~ Max i64
    ];

    let (alpha_y, alpha_z) = if logx <= TABLE[0].0 {
        (TABLE[0].1, TABLE[0].2)
    } else if logx >= TABLE[TABLE.len() - 1].0 {
        (TABLE[TABLE.len() - 1].1, TABLE[TABLE.len() - 1].2)
    } else {
        let mut ay = TABLE[0].1;
        let mut az = TABLE[0].2;
        for i in 0..TABLE.len() - 1 {
            if logx >= TABLE[i].0 && logx < TABLE[i + 1].0 {
                let t = (logx - TABLE[i].0) / (TABLE[i + 1].0 - TABLE[i].0);
                ay = TABLE[i].1 + t * (TABLE[i + 1].1 - TABLE[i].1);
                az = TABLE[i].2 + t * (TABLE[i + 1].2 - TABLE[i].2);
                break;
            }
        }
        (ay, az)
    };

    let x16 = (x as f64).powf(1.0 / 6.0);
    let alpha_y = alpha_y.max(1.0).min(x16);
    let max_alpha_z = (x16 / alpha_y).max(1.0);
    let alpha_z = alpha_z.max(1.0).min(max_alpha_z);

    (alpha_y, alpha_z)
}

// ── x_star ───────────────────────────────────────────────────────────────────

fn get_x_star(x: u64, y: u64) -> u64 {
    let y = std::cmp::max(y, 1);
    let yy = y as u128 * y as u128;
    let x_div_yy = ((x as u128 + yy - 1) / yy) as u64; // ceil_div

    let mut x_star = std::cmp::max(iroot4(x), x_div_yy);
    let sqrt_xy = isqrt(x / y);

    x_star = std::cmp::min(x_star, y);
    x_star = std::cmp::min(x_star, sqrt_xy);
    x_star = std::cmp::max(x_star, 1);

    x_star
}

// ── BigPiTable: O(1) pi lookups for values up to sqrt(x) ────────────────────
// Odd-only sieve with word-granularity prefix sums.
// Separate bits and u32 prefix arrays for 25% less memory (285MB vs 380MB).

struct BigPiTable {
    bits: Vec<u64>,     // prime sieve bits
    prefix: Vec<u32>,   // prefix[w] = popcount(bits[0..w-1]), fits in u32 since pi(3B) < 2^32
}

impl BigPiTable {
    fn new(limit: usize) -> Self {
        if limit < 3 {
            return BigPiTable { bits: vec![], prefix: vec![] };
        }

        let odd_count = (limit - 1) / 2 + 1;
        let nwords = (odd_count + 63) / 64;

        // Parallel segmented sieve construction
        let sqrt_limit = isqrt(limit as u64) as usize;
        let small_sieve = Sieve::new(sqrt_limit);
        let small_primes: Vec<usize> = small_sieve.primes_from(3)
            .take_while(|&p| p <= sqrt_limit).collect();

        // Pre-sieve AND-masks for primes 3, 5, 7
        let masks3 = Self::build_presieve_masks(3);
        let masks5 = Self::build_presieve_masks(5);
        let masks7 = Self::build_presieve_masks(7);

        let seg_words: usize = 4096;
        let seg_odd_count = seg_words * 64;
        let num_segs = (odd_count + seg_odd_count - 1) / seg_odd_count;

        let seg_results: Vec<Vec<u64>> = (0..num_segs).into_par_iter().map(|seg| {
            let start_idx = seg * seg_odd_count;
            let end_idx = std::cmp::min(start_idx + seg_odd_count, odd_count);
            let seg_len = end_idx - start_idx;
            let seg_nw = (seg_len + 63) / 64;

            let mut local = vec![!0u64; seg_nw];
            let excess = seg_nw * 64 - seg_len;
            if excess > 0 { local[seg_nw - 1] &= u64::MAX >> excess; }
            if seg == 0 { local[0] &= !1u64; } // number 1 is not prime

            // Pre-sieve: clear multiples of 3, 5, 7 via word-level masks
            let mut o3 = (1 + 3 - start_idx % 3) % 3;
            let mut o5 = (2 + 5 - start_idx % 5) % 5;
            let mut o7 = (3 + 7 - start_idx % 7) % 7;
            for w in 0..seg_nw {
                unsafe {
                    *local.get_unchecked_mut(w) &=
                        masks3[o3] & masks5[o5] & masks7[o7];
                }
                o3 = (o3 + 2) % 3;
                o5 = (o5 + 1) % 5;
                o7 = (o7 + 6) % 7;
            }

            // Restore prime bits for 3, 5, 7 in first segment
            if seg == 0 {
                local[0] |= (1u64 << 1) | (1u64 << 2) | (1u64 << 3);
            }

            // Cross off remaining primes (>= 11) with per-bit loop
            for &p in &small_primes {
                if p <= 7 { continue; }
                let pp = (p as u64) * (p as u64);
                let low_num = 2 * start_idx + 1;
                let first_num = if pp >= low_num as u64 {
                    pp as usize
                } else {
                    let m = ((low_num + p - 1) / p) * p;
                    if m % 2 == 0 { m + p } else { m }
                };
                let end_num = 2 * (end_idx - 1) + 1;
                if first_num > end_num { continue; }

                let first_odd_idx = (first_num - 1) / 2;
                let local_idx = first_odd_idx - start_idx;
                let mut idx = local_idx;
                while idx < seg_len {
                    unsafe {
                        let w = idx >> 6;
                        let b = idx & 63;
                        *local.get_unchecked_mut(w) &= !(1u64 << b);
                    }
                    idx += p;
                }
            }

            local
        }).collect();

        // Assemble bits and build prefix sums in separate arrays
        let mut all_bits = Vec::with_capacity(nwords);
        for seg in &seg_results {
            all_bits.extend_from_slice(seg);
        }
        all_bits.truncate(nwords);

        let mut prefix = vec![0u32; nwords];
        let mut running = 0u32;
        for i in 0..nwords {
            prefix[i] = running;
            running += all_bits[i].count_ones();
        }

        BigPiTable { bits: all_bits, prefix }
    }

    fn build_presieve_masks(p: usize) -> Vec<u64> {
        let mut masks = vec![!0u64; p];
        for off in 0..p {
            let mut pos = off;
            while pos < 64 {
                masks[off] &= !(1u64 << pos);
                pos += p;
            }
        }
        masks
    }

    #[inline(always)]
    unsafe fn pi_fast(&self, n: usize) -> u64 {
        let odd_idx = (n - 1) >> 1;
        let word = odd_idx >> 6;
        let bit = odd_idx & 63;
        let prefix = *self.prefix.get_unchecked(word) as u64;
        let mask = u64::MAX >> (63 - bit);
        1 + prefix + (*self.bits.get_unchecked(word) & mask).count_ones() as u64
    }

    #[inline]
    fn pi(&self, n: usize) -> u64 {
        if n < 2 { return 0; }
        if n < 3 { return 1; }
        unsafe { self.pi_fast(n) }
    }

    #[inline(always)]
    fn bits_word(&self, w: usize) -> u64 {
        unsafe { *self.bits.get_unchecked(w) }
    }

    #[inline(always)]
    #[cfg(target_arch = "x86_64")]
    fn prefetch(&self, n: usize) {
        if n >= 3 {
            let w = (n - 1) / 2 / 64;
            unsafe {
                _mm_prefetch(self.bits.as_ptr().add(w) as *const i8, _MM_HINT_T0);
                _mm_prefetch(self.prefix.as_ptr().add(w) as *const i8, _MM_HINT_T0);
            }
        }
    }
}

// ── Sigma: 7 correction formulas ─────────────────────────────────────────────

fn compute_sigma(x: u64, y: usize, x_star: usize,
                 _primes: &[u32], pi: &[u32], big_pi: &BigPiTable) -> i64 {
    let pi_limit = pi.len() - 1;
    let a = pi[y] as i64;
    let x13 = icbrt(x) as usize;
    let b = pi[std::cmp::min(x13, pi_limit)] as i64;
    let sqrt_xy = isqrt(x / y as u64) as usize;
    let c = pi[std::cmp::min(sqrt_xy, pi_limit)] as i64;
    let d = pi[std::cmp::min(x_star, pi_limit)] as i64;
    let sqrt_x = isqrt(x) as usize;
    let pi_sx = big_pi.pi(sqrt_x) as i64;

    // Σ₀ = a - 1 + C(π(√x), 2) - C(a, 2)
    let sigma0 = a - 1 + pi_sx * (pi_sx - 1) / 2 - a * (a - 1) / 2;

    // Σ₁ = C(a - b, 2)
    let sigma1 = (a - b) * (a - b - 1) / 2;

    // Σ₂ = a · (b - c - C(c,2) + C(d,2)) with C(n,2) using n*(n-3)/2
    let sigma2 = a * (b - c - c * (c - 3) / 2 + d * (d - 3) / 2);

    // Σ₃ = Σ(n²) - n for n in [d+1, b]
    let sigma3 = b * (b - 1) * (2 * b - 1) / 6 - b
               - (d * (d - 1) * (2 * d - 1) / 6 - d);

    // Σ₄,₅,₆: iterate primes in (x*, x^{1/3}]
    let mut sigma4 = 0i64;
    let mut sigma5 = 0i64;
    let mut sigma6 = 0i64;

    let pi_table_limit = pi.len() - 1;
    let x_star_sieve = Sieve::new(x13);
    for p in x_star_sieve.primes_from(x_star + 1).take_while(|&p| p <= x13) {
        let p64 = p as u64;

        if p <= sqrt_xy {
            // Σ₄: x* < p ≤ √(x/y)
            let xpy = (x / (p64 * y as u64)) as usize;
            sigma4 += pi[std::cmp::min(xpy, pi_table_limit)] as i64;
        } else {
            // Σ₅: √(x/y) < p ≤ x^{1/3}
            let xpp = (x / (p64 * p64)) as usize;
            sigma5 += pi[std::cmp::min(xpp, pi_table_limit)] as i64;
        }

        // Σ₆: x* < p ≤ x^{1/3}
        let sqrt_xp = isqrt(x / p64) as usize;
        let pi_sqrt_xp = pi[std::cmp::min(sqrt_xp, pi_table_limit)] as i64;
        sigma6 += pi_sqrt_xp * pi_sqrt_xp;
    }

    sigma4 *= a;
    sigma6 = -sigma6;

    sigma0 + sigma1 + sigma2 + sigma3 + sigma4 + sigma5 + sigma6
}

// ── Φ₀: ordinary leaves ─────────────────────────────────────────────────────
// Recursive iteration over squarefree numbers ≤ z coprime to first k primes.
// Like DR's S1 but iterates up to z instead of y, with all prime factors ≤ y.

fn phi0_recursive(x: u64, z: usize, b: usize, k: usize,
                  square_free: u64, mu_sign: i64,
                  primes: &[u32], phi_cache: &PhiTinyCache) -> i64 {
    let mut result = 0i64;
    let pi_limit = primes.len() - 1;
    for i in (b + 1)..=pi_limit {
        let next = square_free as u128 * primes[i] as u128;
        if next > z as u128 { break; }
        let next = next as u64;
        result += mu_sign * phi_cache.phi(x / next);
        result += phi0_recursive(x, z, i, k, next, -mu_sign, primes, phi_cache);
    }
    result
}

fn compute_phi0(x: u64, _y: usize, z: usize, k: usize,
                primes: &[u32], phi_cache: &PhiTinyCache) -> i64 {
    let pi_y = primes.len() - 1;
    if pi_y <= k { return phi_cache.phi(x); }

    // Top-level: parallel over first prime index b
    let phi0_base = phi_cache.phi(x);

    let phi0_sum: i64 = ((k + 1)..=pi_y).into_par_iter().map(|b| {
        let p = primes[b] as u64;
        let mut local = -phi_cache.phi(x / p);
        local += phi0_recursive(x, z, b, k, p, 1, primes, phi_cache);
        local
    }).sum();

    phi0_base + phi0_sum
}

// ── B: equivalent to P2 ─────────────────────────────────────────────────────
// Sum of π(x/p) for primes y < p ≤ √x.
// Streaming merge: iterate primes via primesieve and merge with sorted x/p
// values. No bitmap construction, no prefix sums — just a running count.

fn compute_b(x: u64, y: usize, _pi_y: usize, big_pi: &BigPiTable) -> i64 {
    let sqrt_x = isqrt(x) as usize;
    if y >= sqrt_x { return 0; }

    // Collect x/p values as actual numbers in ascending order
    let bp_start = (y + 1) / 2;
    let bp_end = (sqrt_x - 1) / 2;
    if bp_start > bp_end { return 0; }
    let bp_sw = bp_start / 64;
    let bp_ew = bp_end / 64;

    let mut xp_asc: Vec<u64> = Vec::new();
    for word_idx in (bp_sw..=bp_ew).rev() {
        let mut w = big_pi.bits_word(word_idx);
        if word_idx == bp_sw {
            let sb = bp_start % 64;
            if sb > 0 { w &= !((1u64 << sb) - 1); }
        }
        if word_idx == bp_ew {
            let eb = bp_end % 64;
            if eb < 63 { w &= (1u64 << (eb + 1)) - 1; }
        }
        while w != 0 {
            let bit = 63 - w.leading_zeros() as usize;
            w ^= 1u64 << bit;
            let p = (2 * (word_idx * 64 + bit) + 1) as u64;
            xp_asc.push(x / p);
        }
    }

    if xp_asc.is_empty() { return 0; }
    let max_xp = *xp_asc.last().unwrap();

    // Handle xp values within BigPiTable range (≤ sqrt_x) directly
    let split_idx = xp_asc.partition_point(|&v| v <= sqrt_x as u64);
    let mut pre_sum: i64 = 0;
    for i in 0..split_idx {
        pre_sum += big_pi.pi(xp_asc[i] as usize) as i64;
    }

    if split_idx >= xp_asc.len() { return pre_sum; }

    // Streaming merge over [sqrt_x+1, max_xp]
    let base_pi = big_pi.pi(sqrt_x);
    let range_start = sqrt_x as u64 + 1;

    let nthreads = rayon::current_num_threads();
    let nchunks = (nthreads * 8).max(1);
    let range = max_xp - range_start + 1;
    let chunk_size = (range + nchunks as u64 - 1) / nchunks as u64;

    // Assign xp values to chunks based on value ranges
    let chunk_xp_bounds: Vec<usize> = (0..=nchunks).map(|k| {
        if k == 0 { split_idx }
        else {
            let boundary = range_start + k as u64 * chunk_size;
            xp_asc.partition_point(|&v| v < boundary)
        }
    }).collect();

    unsafe { primesieve_set_num_threads(1); }

    // Single-pass: stream primes, merge with xp values, record local sums
    let chunk_results: Vec<(i64, u64, usize)> = (0..nchunks).into_par_iter().map(|k| {
        let xp_lo = chunk_xp_bounds[k];
        let xp_hi = chunk_xp_bounds[k + 1];

        let chunk_start = range_start + k as u64 * chunk_size;
        let chunk_end = std::cmp::min(range_start + (k as u64 + 1) * chunk_size - 1, max_xp);

        let mut ps_iter = PrimesieveIterator::new();
        ps_iter.jump_to(chunk_start, chunk_end + 1);
        let mut p = ps_iter.next_prime();
        let mut running: u64 = 0;
        let mut local_sum: i64 = 0;

        // Merge primes with xp values
        for i in xp_lo..xp_hi {
            let v = unsafe { *xp_asc.get_unchecked(i) };
            while p <= v {
                running += 1;
                p = ps_iter.next_prime();
            }
            local_sum += running as i64;
        }

        // Count remaining primes in chunk (for prefix correction)
        while p <= chunk_end {
            running += 1;
            p = ps_iter.next_prime();
        }

        (local_sum, running, xp_hi - xp_lo)
    }).collect();

    // Combine with prefix corrections
    let mut total_sum: i64 = pre_sum;
    let mut prefix_pi: u64 = base_pi;
    for &(local_sum, chunk_primes, num_lookups) in &chunk_results {
        total_sum += local_sum + prefix_pi as i64 * num_lookups as i64;
        prefix_pi += chunk_primes;
    }

    total_sum
}

// ── AC: combined A + C formulas ──────────────────────────────────────────────
// Uses BigPiTable for O(1) lookups of π(x/pq) up to √x.
// C1: recursive Möbius (few iterations)
// C2: easy leaves with pi table (π(√z) < b ≤ π(x*))
// A:  simplest easy leaves (π(x*) < b ≤ π(x^{1/3}))

fn c1_recursive(xp: u64, b: usize, i: usize, pi_y: usize,
                m: u64, min_m: usize, max_m: usize,
                mu_sign: i64, primes: &[u32], pi: &[u32]) -> i64 {
    let mut sum = 0i64;
    let y_limit = pi.len() - 1;
    for j in (i + 1)..=pi_y {
        let next = m as u128 * primes[j] as u128;
        if next > max_m as u128 { return sum; }
        let next = next as u64;

        if next as usize > min_m {
            let xpm = (xp / next) as usize;
            let phi_xpm = pi[std::cmp::min(xpm, y_limit)] as i64 - b as i64 + 2;
            sum += phi_xpm * mu_sign;
        }

        sum += c1_recursive(xp, b, j, pi_y, next, min_m, max_m,
                           -mu_sign, primes, pi);
    }
    sum
}

#[inline(always)]
fn fast_div(n: u64, d: u64, recip_d: u64) -> u64 {
    let q = ((n as u128 * recip_d as u128) >> 64) as u64;
    q + (n - q.wrapping_mul(d) >= d) as u64
}

fn compute_ac(x: u64, y: usize, z: usize, k: usize, x_star: usize,
              primes: &[u32], pi: &[u32], big_pi: &BigPiTable) -> i64 {
    let pi_limit = pi.len() - 1;
    let x13 = icbrt(x) as usize;
    let sqrt_x = isqrt(x) as usize;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_y = pi[y] as usize;
    let pi_sqrtz = pi[std::cmp::min(sqrt_z, pi_limit)] as usize;
    let pi_x_star = pi[std::cmp::min(x_star, pi_limit)] as usize;
    let pi_x13 = pi[std::cmp::min(x13, pi_limit)] as usize;
    let pi_root3_xz = pi[std::cmp::min(icbrt(x / z as u64) as usize, pi_limit)] as usize;
    let pi_root3_xy = pi[std::cmp::min(icbrt(x / y as u64) as usize, pi_limit)] as usize;

    // ── C1: recursive Möbius, sequential (very few iterations) ───────────
    let mut c1 = 0i64;
    let min_c1_b = std::cmp::max(k, pi_root3_xz) + 1;
    for b in min_c1_b..=pi_sqrtz {
        if b >= primes.len() { break; }
        let prime = primes[b] as u64;
        let xp = x / prime;
        let max_m = std::cmp::min((xp / prime) as usize, z);
        let min_m_val = std::cmp::max((xp / (prime * prime)) as usize, z / prime as usize);
        let min_m_val = std::cmp::min(min_m_val, max_m);

        c1 -= c1_recursive(xp, b, b, pi_y, 1, min_m_val, max_m,
                           -1, primes, pi);
    }

    // ── C2 + A: round-based processing for cache-friendly BigPiTable access ──
    // Instead of parallel over b (random BigPiTable access), process BigPiTable
    // in segments from high to low, keeping working set in L2/L3 cache.
    // Precompute reciprocals
    let recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    let min_c2_b = std::cmp::max(k, std::cmp::max(pi_root3_xy, pi_sqrtz)) + 1;

    // Build per-b info for C2 and A
    // kind: 0 = C2 (contribute pi-b+2), 1 = A (x/pq>=y: 1×pi), 2 = A (x/pq<y: 2×pi)
    // We track ascending l pointer; xpq decreases as l increases
    struct BLookup {
        b: usize,
        xp: u64,
        l_cur: usize,
        l_max: usize,
        y_boundary_l: usize,  // for A: l > y_boundary_l means coefficient becomes 2
        is_c2: bool,
    }

    let mut b_lookups: Vec<BLookup> = Vec::new();

    // C2 b values: iterate l from pi_min+1 (ascending) to pi_max
    for b in min_c2_b..=pi_x_star {
        if b >= primes.len() { continue; }
        let prime = primes[b] as u64;
        let xp = x / prime;
        let max_m = std::cmp::min(std::cmp::min((xp / prime) as usize, y),
                                   (xp / std::cmp::max(1, prime) as u64) as usize);
        let min_m_val = std::cmp::max(
            std::cmp::max((xp / (prime * prime)) as usize, prime as usize), 1);
        let min_m_val = std::cmp::min(min_m_val, max_m);
        if max_m <= min_m_val { continue; }
        let l_max = pi[std::cmp::min(max_m, pi_limit)] as usize;
        let l_min = pi[std::cmp::min(min_m_val, pi_limit)] as usize + 1;
        if l_min > l_max { continue; }
        b_lookups.push(BLookup { b, xp, l_cur: l_min, l_max, y_boundary_l: usize::MAX, is_c2: true });
    }

    // A b values: iterate i from min_i (ascending) to max_i
    for b in (pi_x_star + 1)..=pi_x13 {
        if b >= primes.len() { continue; }
        let prime = primes[b] as u64;
        let xp = x / prime;
        let sqrt_xp = isqrt(xp) as usize;
        let max_2nd = std::cmp::min(sqrt_xp, y);
        let min_2nd = std::cmp::max(prime as usize, 1);
        if max_2nd <= min_2nd { continue; }
        let max_i = pi[std::cmp::min(max_2nd, pi_limit)] as usize;
        let min_i = pi[std::cmp::min(min_2nd, pi_limit)] as usize + 1;
        if min_i > max_i { continue; }
        let xp_over_y = (xp / y as u64) as usize;
        let y_boundary_l = pi[std::cmp::min(std::cmp::min(xp_over_y, max_2nd), pi_limit)] as usize;
        b_lookups.push(BLookup { b, xp, l_cur: min_i, l_max: max_i, y_boundary_l, is_c2: false });
    }

    let c2_a_sum: i64 = if b_lookups.is_empty() { 0 } else {
        let primes_len = primes.len();

        // Segmented AC: process BigPiTable in L1-cache-sized segments.
        // Smaller segments = fewer L3/DRAM misses for pi() lookups.
        let seg_pairs: usize = 130_000;
        let total_pairs = big_pi.bits.len();
        let num_segs = (total_pairs + seg_pairs - 1) / seg_pairs;

        // Each pair covers 128 numbers (64 odd numbers per word * 2 numbers per odd)
        // Segment s covers numbers [s*seg_pairs*128, (s+1)*seg_pairs*128)
        let numbers_per_seg = seg_pairs * 128;

        let mut total: i64 = 0;
        for seg in (0..num_segs).rev() {
            let n_lo = if seg == 0 { 0usize } else { seg * numbers_per_seg };
            let n_hi = std::cmp::min((seg + 1) * numbers_per_seg - 1, sqrt_x);

            let seg_sum: i64 = b_lookups.par_iter().map(|info| {
                // Find l range where n_lo <= xp/primes[l] <= n_hi
                // Using pi table for O(1) lookup instead of partition_point
                let l_lo = if seg == num_segs - 1 {
                    info.l_cur // highest segment includes clamped values
                } else {
                    // First l where primes[l] > xp/(n_hi+1), i.e. xp/primes[l] < n_hi+1
                    let thresh = std::cmp::min(
                        (info.xp / (n_hi as u64 + 1)) as usize, pi_limit);
                    let l_candidate = pi[thresh] as usize + 1;
                    std::cmp::max(l_candidate, info.l_cur)
                };

                let l_hi = if n_lo <= 1 {
                    info.l_max
                } else {
                    // Last l where primes[l] <= xp/n_lo
                    let thresh_raw = info.xp / n_lo as u64;
                    if thresh_raw == 0 { return 0; }
                    let thresh = std::cmp::min(thresh_raw as usize, pi_limit);
                    let l_candidate = pi[thresh] as usize;
                    std::cmp::min(l_candidate, info.l_max)
                };

                let eff_lo = std::cmp::max(l_lo, info.l_cur);
                let eff_hi = std::cmp::min(l_hi, std::cmp::min(info.l_max, primes_len - 1));
                if eff_lo > eff_hi || eff_lo >= primes_len { return 0; }

                let mut local = 0i64;
                let mut l = eff_lo;

                while l + 3 <= eff_hi {
                    let xpq0 = fast_div(info.xp, primes[l] as u64, recip[l]) as usize;
                    let xpq1 = fast_div(info.xp, primes[l+1] as u64, recip[l+1]) as usize;
                    let xpq2 = fast_div(info.xp, primes[l+2] as u64, recip[l+2]) as usize;
                    let xpq3 = fast_div(info.xp, primes[l+3] as u64, recip[l+3]) as usize;

                    unsafe {
                        if info.is_c2 {
                            local += (big_pi.pi_fast(xpq0) as i64 - info.b as i64 + 2)
                                   + (big_pi.pi_fast(xpq1) as i64 - info.b as i64 + 2)
                                   + (big_pi.pi_fast(xpq2) as i64 - info.b as i64 + 2)
                                   + (big_pi.pi_fast(xpq3) as i64 - info.b as i64 + 2);
                        } else {
                            let p0 = big_pi.pi_fast(xpq0) as i64;
                            let p1 = big_pi.pi_fast(xpq1) as i64;
                            let p2 = big_pi.pi_fast(xpq2) as i64;
                            let p3 = big_pi.pi_fast(xpq3) as i64;
                            if l + 3 <= info.y_boundary_l {
                                local += p0 + p1 + p2 + p3;
                            } else if l > info.y_boundary_l {
                                local += 2 * (p0 + p1 + p2 + p3);
                            } else {
                                for (ll, pv) in [(l, p0), (l+1, p1), (l+2, p2), (l+3, p3)] {
                                    local += if ll <= info.y_boundary_l { pv } else { 2 * pv };
                                }
                            }
                        }
                    }
                    l += 4;
                }

                while l <= eff_hi {
                    let xpq = fast_div(info.xp, primes[l] as u64, recip[l]) as usize;
                    let pi_val = unsafe { big_pi.pi_fast(xpq) } as i64;
                    if info.is_c2 {
                        local += pi_val - info.b as i64 + 2;
                    } else if l <= info.y_boundary_l {
                        local += pi_val;
                    } else {
                        local += 2 * pi_val;
                    }
                    l += 1;
                }

                local
            }).sum();

            total += seg_sum;
        }

        total
    };

    c1 + c2_a_sum
}

// ── Mod-30 wheel sieve constants ─────────────────────────────────────────────
// 8 residues coprime to 30 per group of 30 numbers.
// Benefits: 1.875× fewer sieve operations than odd-only (8/30 vs 1/2 density).

const WHEEL30_RESIDUES: [usize; 8] = [1, 7, 11, 13, 17, 19, 23, 29];

// Map: n % 30 → index in WHEEL30_RESIDUES (255 if not coprime to 30)
const MOD30_TO_IDX: [u8; 30] = [
    255, 0, 255, 255, 255, 255, 255, 1, 255, 255,
    255, 2, 255, 3, 255, 255, 255, 4, 255, 5,
    255, 255, 255, 6, 255, 255, 255, 255, 255, 7,
];

// Map: for n with offset r = (n - low) % 30, which bit position is the largest
// coprime-to-30 number ≤ n? -1 means previous group's bit 7.
const FLOOR_WHEEL_BIT: [i8; 30] = [
    -1, 0, 0, 0, 0, 0, 0, 1, 1, 1,
     1, 2, 2, 3, 3, 3, 3, 4, 4, 5,
     5, 5, 5, 6, 6, 6, 6, 6, 6, 7,
];

// Number of coprime residues in [1, r] for each r in 0..30
const COPRIME_COUNT_30: [u8; 31] = [
    0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 4, 4, 4,
    4, 5, 5, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 8, 8,
];

// Wheel cross-off tables (kept for reference / future use with byte-level stepping)
#[allow(dead_code)]
const WHEEL_GAPS: [usize; 8] = [6, 4, 2, 4, 2, 4, 6, 2];
#[allow(dead_code)]
const WHEEL_BITS: [[u8; 8]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7], // p ≡ 1
    [1, 5, 4, 0, 7, 3, 2, 6], // p ≡ 7
    [2, 4, 0, 6, 1, 7, 3, 5], // p ≡ 11
    [3, 0, 6, 5, 2, 1, 7, 4], // p ≡ 13
    [4, 7, 1, 2, 5, 6, 0, 3], // p ≡ 17
    [5, 3, 7, 1, 6, 0, 4, 2], // p ≡ 19
    [6, 2, 3, 7, 0, 4, 5, 1], // p ≡ 23
    [7, 6, 5, 4, 3, 2, 1, 0], // p ≡ 29
];

// WHEEL_CORR[r_idx][step]: byte advance correction (added to p/30 * gap[step])
#[allow(dead_code)]
const WHEEL_CORR: [[usize; 8]; 8] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // p ≡ 1
    [1, 1, 1, 0, 1, 1, 1, 1], // p ≡ 7
    [2, 2, 0, 2, 0, 2, 2, 1], // p ≡ 11
    [3, 1, 1, 2, 1, 1, 3, 1], // p ≡ 13
    [3, 3, 1, 2, 1, 3, 3, 1], // p ≡ 17
    [4, 2, 2, 2, 2, 2, 4, 1], // p ≡ 19
    [5, 3, 1, 4, 1, 3, 5, 1], // p ≡ 23
    [6, 4, 2, 4, 2, 4, 6, 1], // p ≡ 29
];

// Find smallest number >= n that is coprime to 30
#[inline(always)]
#[allow(dead_code)]
fn next_coprime30(n: usize) -> usize {
    let r = n % 30;
    let base = n - r;
    for &res in &WHEEL30_RESIDUES {
        if res >= r { return base + res; }
    }
    base + 30 + 1
}

// Convert number n to wheel bit position in segment starting at low (low must be multiple of 30)
// Returns the position of the largest coprime-to-30 number ≤ n
#[inline(always)]
fn num_to_wheel_pos(n: usize, low: usize) -> usize {
    let offset = n - low;
    let group = offset / 30;
    let r = offset % 30;
    let bit = FLOOR_WHEEL_BIT[r];
    if bit >= 0 {
        group * 8 + bit as usize
    } else {
        (group - 1) * 8 + 7
    }
}

// Number of wheel bits needed to cover n numbers starting from a 30-aligned boundary
#[inline(always)]
fn wheel_bit_count(n: usize) -> usize {
    (n / 30) * 8 + COPRIME_COUNT_30[n % 30] as usize
}

// ── D: hard special leaves via segmented sieve ───────────────────────────────
// Similar to V6's S2_hard but with Gourdon bounds:
//   Type 1 (b ≤ π(√z)): squarefree m ≤ z, μ(m)≠0, lpf(m)>p, all factors ≤ y
//   Type 2 (π(√z) < b ≤ π(x*)): prime pairs (p, q)

struct BitSieve {
    bits: Vec<u64>,
    len: usize,
    total: i64,
}

impl BitSieve {
    fn new(max_len: usize) -> Self {
        BitSieve { bits: vec![0u64; (max_len + 63) / 64], len: 0, total: 0 }
    }

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
        let first_mask = if b0 < 63 { u64::MAX << (b0 + 1) } else { 0 };
        cnt += (unsafe { *self.bits.get_unchecked(w0) } & first_mask).count_ones() as u64;
        for i in (w0 + 1)..w1 {
            cnt += unsafe { *self.bits.get_unchecked(i) }.count_ones() as u64;
        }
        let mask = (2u64 << b1) - 1;
        cnt += (unsafe { *self.bits.get_unchecked(w1) } & mask).count_ones() as u64;
        cnt as i64
    }

    fn count_total(&self) -> i64 { self.total }
}

struct PreSieveTemplate {
    bits: Vec<u64>,
    period: usize, // period in groups of 30 (bytes)
}

impl PreSieveTemplate {
    fn new(primes: &[u32], c: usize) -> Self {
        // For wheel-30: primes 2,3,5 are implicit. Pre-sieve primes[4..=c] = {7,11,13,17}
        let first_b = 4; // primes[4] = 7
        if first_b > c {
            // No primes to pre-sieve; return all-ones template
            return PreSieveTemplate { bits: vec![u64::MAX; 2], period: 1 };
        }
        let mut period: usize = 1;
        for b in first_b..=c { period *= primes[b] as usize; }
        // period in groups-of-30 (bytes): since gcd(product, 30) = 1, period is also in bytes

        let nbits = period * 8;
        let nwords = (nbits + 63) / 64;
        let double_nwords = nwords + (nwords + 1); // extra for unaligned reads
        let mut bits = vec![u64::MAX; double_nwords];

        // Mark composites: for each pre-sieve prime p, and each coprime residue r,
        // p*r is a composite. Clear that bit and repeat every p bytes.
        for b in first_b..=c {
            let p = primes[b] as usize;
            for &r in &WHEEL30_RESIDUES {
                let composite = p * r;
                let byte0 = composite / 30;
                let bit_in_byte = MOD30_TO_IDX[composite % 30] as usize;
                let mut byte = byte0;
                while byte < period * 2 + p {
                    let pos = byte * 8 + bit_in_byte;
                    if pos < double_nwords * 64 {
                        bits[pos / 64] &= !(1u64 << (pos % 64));
                    }
                    byte += p;
                }
            }
        }

        PreSieveTemplate { bits, period }
    }

    #[inline]
    fn get_word(&self, start_bit: usize) -> u64 {
        let w = start_bit / 64;
        let bit_off = start_bit % 64;
        if bit_off == 0 {
            unsafe { *self.bits.get_unchecked(w) }
        } else {
            let lo = unsafe { *self.bits.get_unchecked(w) } >> bit_off;
            let hi = unsafe { *self.bits.get_unchecked(w + 1) } << (64 - bit_off);
            lo | hi
        }
    }

    fn init_sieve(&self, sieve: &mut BitSieve, low: usize, wheel_seg_bits: usize) {
        sieve.len = wheel_seg_bits;
        let nwords = (wheel_seg_bits + 63) / 64;
        let tpl_start_bit = ((low / 30) % self.period) * 8;
        let tpl_period_bits = self.period * 8;
        let mut tpl_pos = tpl_start_bit;
        let mut total = 0i64;
        for w in 0..nwords {
            let word = self.get_word(tpl_pos);
            unsafe { *sieve.bits.get_unchecked_mut(w) = word; }
            total += word.count_ones() as i64;
            tpl_pos += 64;
            if tpl_pos >= tpl_period_bits { tpl_pos -= tpl_period_bits; }
        }
        let excess = nwords * 64 - wheel_seg_bits;
        if excess > 0 {
            let last = unsafe { *sieve.bits.get_unchecked(nwords - 1) };
            let masked = last & (u64::MAX >> excess);
            total -= (last.count_ones() as i64) - (masked.count_ones() as i64);
            unsafe { *sieve.bits.get_unchecked_mut(nwords - 1) = masked; }
        }
        sieve.total = total;
    }
}

fn cross_off_sieve(sieve: &mut BitSieve, prime: usize, low: usize, high: usize,
                   _wheel_seg_len: usize) {
    let p = prime;
    let wheel_seg_bits = sieve.len;
    let bits = sieve.bits.as_mut_ptr();
    let mut delta = 0i64;
    let step = p * 8; // uniform step in wheel-bit-space for same residue

    for r_idx in 0..8 {
        let r = WHEEL30_RESIDUES[r_idx];
        let composite_residue = (p * r) % 30;
        let bit_in_byte = MOD30_TO_IDX[composite_residue] as usize;

        // First composite p*q in [max(low+1, p), high) where q ≡ r (mod 30)
        let first_q = {
            let min_q = std::cmp::max((low + p) / p, 1);
            let base = (min_q / 30) * 30;
            if base + r >= min_q { base + r } else { base + 30 + r }
        };
        let first_composite = p * first_q;
        if first_composite >= high { continue; }

        let base_pos = ((first_composite - low) / 30) * 8 + bit_in_byte;
        let mut pos = base_pos;

        // 4x unrolled word-level cross-off (same pattern as old odd-only code)
        while pos + step * 3 < wheel_seg_bits {
            unsafe {
                let w0 = pos >> 6; let b0 = pos & 63;
                let old0 = *bits.add(w0); delta += ((old0 >> b0) & 1) as i64;
                *bits.add(w0) = old0 & !(1u64 << b0);
                let pos1 = pos + step;
                let w1 = pos1 >> 6; let b1 = pos1 & 63;
                let old1 = *bits.add(w1); delta += ((old1 >> b1) & 1) as i64;
                *bits.add(w1) = old1 & !(1u64 << b1);
                let pos2 = pos + step * 2;
                let w2 = pos2 >> 6; let b2 = pos2 & 63;
                let old2 = *bits.add(w2); delta += ((old2 >> b2) & 1) as i64;
                *bits.add(w2) = old2 & !(1u64 << b2);
                let pos3 = pos + step * 3;
                let w3 = pos3 >> 6; let b3 = pos3 & 63;
                let old3 = *bits.add(w3); delta += ((old3 >> b3) & 1) as i64;
                *bits.add(w3) = old3 & !(1u64 << b3);
            }
            pos += step * 4;
        }
        while pos < wheel_seg_bits {
            unsafe {
                let w = pos >> 6; let b = pos & 63;
                let old = *bits.add(w); delta += ((old >> b) & 1) as i64;
                *bits.add(w) = old & !(1u64 << b);
            }
            pos += step;
        }
    }

    sieve.total -= delta;
}

// Precomputed valid m values for D Type 1 (squarefree, y-smooth, lpf > min threshold)
#[derive(Clone, Copy)]
#[repr(C)]
struct ValidM {
    m: u32,
    lpf: u16,    // clamped at u16::MAX (primes[b] always < u16::MAX for Type 1)
    mu_val: i8,
    _pad: u8,
    recip_m: u64, // precomputed (1 << 64) / m for fast division
}

fn compute_d(x: u64, y: usize, z: usize, k: usize, x_star: usize,
             primes: &[u32], pi: &[u32],
             mu: &[i8], lpf: &[i32], y_smooth: &[bool]) -> i64 {
    if z == 0 { return 0; }

    let xz = (x / z as u64) as usize;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_limit = pi.len() - 1;
    let pi_sqrtz = pi[std::cmp::min(sqrt_z, pi_limit)] as usize;
    let pi_x_star = pi[std::cmp::min(x_star, pi_limit)] as usize;
    let nprimes = primes.len();
    let c = k;

    if c >= pi_x_star { return 0; }

    let template = PreSieveTemplate::new(primes, std::cmp::min(c, nprimes - 1));

    let prime_recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    // Precompute ValidM list for Type 1 leaves
    let min_lpf_threshold = if c + 1 < primes.len() { primes[c + 1] as i32 } else { i32::MAX };
    let valid_m_list: Vec<ValidM> = if pi_sqrtz > c {
        (1..=z).filter(|&m| m < mu.len() && mu[m] != 0 && lpf[m] > min_lpf_threshold && y_smooth[m])
            .map(|m| ValidM {
                m: m as u32,
                lpf: std::cmp::min(lpf[m] as u32, u16::MAX as u32) as u16,
                mu_val: mu[m],
                _pad: 0,
                recip_m: ((1u128 << 64) / m as u128) as u64,
            }).collect()
    } else { vec![] };

    // Build sparse index for O(1) initial lookup into valid_m_list
    const VM_STRIDE: usize = 64;
    let vm_index: Vec<u32> = if !valid_m_list.is_empty() {
        let max_m = valid_m_list.last().unwrap().m as usize;
        let index_len = max_m / VM_STRIDE + 2;
        let mut idx = vec![valid_m_list.len() as u32; index_len];
        let mut vi = 0usize;
        for bucket in 0..index_len {
            let bucket_start = bucket * VM_STRIDE;
            while vi < valid_m_list.len() && (valid_m_list[vi].m as usize) < bucket_start {
                vi += 1;
            }
            idx[bucket] = vi as u32;
        }
        idx
    } else { vec![] };

    let target_segs = rayon::current_num_threads() * 32;
    let seg_cap = std::env::var("D_SEG_CAP").ok()
        .and_then(|s| s.parse::<u32>().ok()).unwrap_or(20);
    // Round segment_size to multiple of 30 for wheel-30 alignment
    let segment_size_raw = std::cmp::max(
        std::cmp::min(xz / std::cmp::max(target_segs, 1), 1usize << seg_cap),
        1 << 17
    ).next_power_of_two();
    let segment_size = (segment_size_raw / 30) * 30;
    let segment_size = segment_size.max(30);
    let num_segments = if xz == 0 { 1 } else { (xz / segment_size) + 1 };

    if num_segments <= 2 {
        return compute_d_serial(x, y, z, k, x_star, xz, primes, pi,
                                mu, lpf, y_smooth, pi_sqrtz, pi_x_star,
                                segment_size, &template, &prime_recip,
                                &valid_m_list);
    }

    // Work-balanced chunk assignment: early segments have much higher cur_max_b
    let nchunks = std::cmp::min(num_segments, rayon::current_num_threads() * 32);
    let work_per_seg: Vec<usize> = (0..num_segments).map(|seg_idx| {
        let low = std::cmp::max(seg_idx * segment_size, 1);
        std::cmp::min(
            pi[std::cmp::min(isqrt(x / low as u64) as usize, pi_limit)] as usize,
            pi_x_star)
    }).collect();
    let total_work: usize = work_per_seg.iter().sum();
    let target_work = total_work / nchunks;

    let mut chunk_bounds: Vec<usize> = Vec::with_capacity(nchunks + 1);
    chunk_bounds.push(0);
    let mut cum_work = 0usize;
    for (i, &w) in work_per_seg.iter().enumerate() {
        cum_work += w;
        if cum_work >= target_work * chunk_bounds.len() && chunk_bounds.len() < nchunks {
            chunk_bounds.push(i + 1);
        }
    }
    chunk_bounds.push(num_segments);
    let actual_nchunks = chunk_bounds.len() - 1;

    let results: Vec<(i64, Vec<i64>, Vec<i64>, usize)> = (0..actual_nchunks).into_par_iter().map(|tid| {
        let seg_start = chunk_bounds[tid];
        let seg_end = chunk_bounds[tid + 1];

        // Pre-compute max_b for this chunk to size phi/coeff vectors
        let chunk_max_b = (seg_start..seg_end).map(|seg_idx| {
            let low = std::cmp::max(seg_idx * segment_size, 1);
            if low > xz { return 0; }
            std::cmp::min(
                pi[std::cmp::min(isqrt(x / low as u64) as usize, pi_limit)] as usize,
                pi_x_star)
        }).max().unwrap_or(0);
        let vec_size = std::cmp::min(chunk_max_b + 1, nprimes);

        let max_wheel_bits = wheel_bit_count(segment_size);
        let mut sieve = BitSieve::new(max_wheel_bits);
        let mut phi = vec![0i64; vec_size];
        let mut d_local = 0i64;
        let mut coeff = vec![0i64; vec_size];
        let mut max_b_seen: usize = 0;

        for seg_idx in seg_start..seg_end {
            let low = seg_idx * segment_size;
            if low > xz { break; }
            let high = std::cmp::min(low + segment_size, xz + 1);
            let wheel_seg_bits = wheel_bit_count(high - low);
            let low1 = std::cmp::max(low, 1);
            if wheel_seg_bits == 0 { break; }

            template.init_sieve(&mut sieve, low, wheel_seg_bits);

            // Clear bit for number 1 in first segment (1 is coprime to 30 but not a prime for phi)
            // Actually, for phi sieve, 1 SHOULD be counted (phi counts numbers coprime to first b primes)
            // No adjustment needed.

            let cur_max_b = std::cmp::min(
                pi[std::cmp::min(isqrt(x / low1 as u64) as usize, pi_limit)] as usize,
                pi_x_star);
            if cur_max_b > max_b_seen { max_b_seen = cur_max_b; }

            let mut b = c + 1;

            // Type 1: b ≤ π(√z), squarefree m leaves (using precomputed ValidM list)
            while b <= std::cmp::min(pi_sqrtz, cur_max_b) && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, z);
                let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, z);
                let min_m = std::cmp::max(xp_high, z / prime as usize);
                let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);

                if prime as usize >= max_m { break; }

                if min_m < max_m {
                    // Use sparse index for fast initial lookup, then short binary search
                    let vm_start = {
                        let bucket = std::cmp::min(min_m / VM_STRIDE, vm_index.len() - 1);
                        let hint = vm_index[bucket] as usize;
                        let search_end = if bucket + 2 < vm_index.len() {
                            vm_index[bucket + 2] as usize
                        } else { valid_m_list.len() };
                        let search_end = std::cmp::min(search_end, valid_m_list.len());
                        let slice = &valid_m_list[hint..search_end];
                        hint + slice.partition_point(|v| (v.m as usize) <= min_m)
                    };
                    let vm_end = {
                        let bucket = std::cmp::min(max_m / VM_STRIDE, vm_index.len() - 1);
                        let hint = vm_index[bucket] as usize;
                        let search_end = if bucket + 2 < vm_index.len() {
                            vm_index[bucket + 2] as usize
                        } else { valid_m_list.len() };
                        let search_end = std::cmp::min(search_end, valid_m_list.len());
                        let slice = &valid_m_list[hint..search_end];
                        hint + slice.partition_point(|v| (v.m as usize) <= max_m)
                    };

                    let mut prev_pos: Option<usize> = None;
                    let mut running_count: i64 = 0;
                    for v in valid_m_list[vm_start..vm_end].iter().rev() {
                        if prime < v.lpf as u64 {
                            let xpm = fast_div(x_div_prime, v.m as u64,
                                v.recip_m) as usize;
                            if xpm > low && xpm < high {
                                let pos = num_to_wheel_pos(xpm, low);
                                let count = match prev_pos {
                                    None => { running_count = sieve.count(pos); running_count }
                                    Some(pp) if pos == pp => running_count,
                                    Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
                                };
                                d_local -= v.mu_val as i64 * (phi[b] + count);
                                coeff[b] -= v.mu_val as i64;
                                prev_pos = Some(pos);
                            } else if xpm == low {
                                d_local -= v.mu_val as i64 * phi[b];
                                coeff[b] -= v.mu_val as i64;
                            }
                        }
                    }
                }

                phi[b] += sieve.count_total();
                cross_off_sieve(&mut sieve, prime as usize, low, high, wheel_seg_bits);
                b += 1;
            }

            // Type 2: π(√z) < b ≤ π(x*), prime pair leaves
            while b <= cur_max_b && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, y);
                let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, y);
                let min_m = std::cmp::max(xp_high, prime as usize);
                let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);
                let mut l = pi[std::cmp::min(max_m, pi_limit)] as usize;

                if l < nprimes && prime as usize >= primes[l] as usize { break; }

                let mut prev_pos: Option<usize> = None;
                let mut running_count: i64 = 0;
                while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                    let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                    if xpq > low && xpq < high {
                        let pos = num_to_wheel_pos(xpq, low);
                        let count = match prev_pos {
                            Some(pp) if pos == pp => running_count,
                            Some(pp) if pos > pp => {
                                running_count += sieve.count_delta(pp, pos);
                                running_count
                            }
                            _ => { running_count = sieve.count(pos); running_count }
                        };
                        d_local += phi[b] + count;
                        coeff[b] += 1;
                        prev_pos = Some(pos);
                    } else if xpq == low {
                        d_local += phi[b];
                        coeff[b] += 1;
                    } else if xpq >= high {
                        break; // xpq only increases as l decreases
                    }
                    l -= 1;
                }

                phi[b] += sieve.count_total();
                cross_off_sieve(&mut sieve, prime as usize, low, high, wheel_seg_bits);
                b += 1;
            }
        }

        (d_local, phi, coeff, max_b_seen)
    }).collect();

    // Correction pass for phi offsets across chunk boundaries
    let mut d = results[0].0;
    let mut prefix_phi = results[0].1.clone();

    for kk in 1..results.len() {
        let (d_local, ref phi_total, ref coeff, max_b_seen) = results[kk];
        let limit = std::cmp::min(max_b_seen + 1, nprimes);
        let mut correction = 0i64;
        for bb in 0..limit {
            correction += prefix_phi[bb] * coeff[bb];
        }
        d += d_local + correction;
        for bb in 0..limit {
            prefix_phi[bb] += phi_total[bb];
        }
    }

    d
}

fn compute_d_serial(x: u64, y: usize, z: usize, k: usize, _x_star: usize, xz: usize,
                    primes: &[u32], pi: &[u32],
                    _mu: &[i8], _lpf: &[i32], _y_smooth: &[bool],
                    pi_sqrtz: usize, pi_x_star: usize,
                    segment_size: usize, template: &PreSieveTemplate,
                    prime_recip: &[u64], valid_m_list: &[ValidM]) -> i64 {
    let nprimes = primes.len();
    let pi_limit = pi.len() - 1;
    let c = k;
    let mut phi: Vec<i64> = vec![0i64; nprimes];
    let max_wheel_bits = wheel_bit_count(segment_size);
    let mut sieve = BitSieve::new(max_wheel_bits);
    let mut d: i64 = 0;
    let mut low: usize = 0;


    while low <= xz {
        let high = std::cmp::min(low + segment_size, xz + 1);
        let wheel_seg_bits = wheel_bit_count(high - low);
        let low1 = std::cmp::max(low, 1);
        if wheel_seg_bits == 0 { break; }

        template.init_sieve(&mut sieve, low, wheel_seg_bits);

        let cur_max_b = std::cmp::min(
            pi[std::cmp::min(isqrt(x / low1 as u64) as usize, pi_limit)] as usize,
            pi_x_star);

        let mut b = c + 1;

        // Type 1
        while b <= std::cmp::min(pi_sqrtz, cur_max_b) && b < nprimes {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, z);
            let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, z);
            let min_m = std::cmp::max(xp_high, z / prime as usize);
            let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);

            if prime as usize >= max_m { break; }

            if min_m < max_m {
                let vm_start = valid_m_list.partition_point(|v| (v.m as usize) <= min_m);
                let vm_end = valid_m_list.partition_point(|v| (v.m as usize) <= max_m);

                let mut prev_pos: Option<usize> = None;
                let mut running_count: i64 = 0;
                for v in valid_m_list[vm_start..vm_end].iter().rev() {
                    if prime < v.lpf as u64 {
                        let xpm = fast_div(x_div_prime, v.m as u64,
                            v.recip_m) as usize;
                        if xpm > low && xpm < high {
                            let pos = num_to_wheel_pos(xpm, low);
                            let count = match prev_pos {
                                None => { running_count = sieve.count(pos); running_count }
                                Some(pp) if pos == pp => running_count,
                                Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
                            };
                            d -= v.mu_val as i64 * (phi[b] + count);
                            prev_pos = Some(pos);
                        } else if xpm == low {
                            d -= v.mu_val as i64 * phi[b];
                        }
                    }
                }
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            cross_off_sieve(&mut sieve, p, low, high, wheel_seg_bits);
            b += 1;
        }

        // Type 2
        while b <= cur_max_b && b < nprimes {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, y);
            let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, y);
            let min_m = std::cmp::max(xp_high, prime as usize);
            let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);
            let mut l = pi[std::cmp::min(max_m, pi_limit)] as usize;

            if l < nprimes && prime as usize >= primes[l] as usize { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                if xpq > low && xpq < high {
                    let pos = num_to_wheel_pos(xpq, low);
                    let count = match prev_pos {
                        Some(pp) if pos == pp => running_count,
                        Some(pp) if pos > pp => {
                            running_count += sieve.count_delta(pp, pos);
                            running_count
                        }
                        _ => { running_count = sieve.count(pos); running_count }
                    };
                    d += phi[b] + count;
                    prev_pos = Some(pos);
                } else if xpq == low {
                    d += phi[b];
                } else if xpq >= high {
                    break; // xpq only increases as l decreases
                }
                l -= 1;
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            cross_off_sieve(&mut sieve, p, low, high, wheel_seg_bits);
            b += 1;
        }

        low += segment_size;
    }

    d
}

// ── Main counting function ───────────────────────────────────────────────────

fn count_primes(x: u64) -> u64 {
    if x < 2 { return 0; }
    if x <= 10_000 {
        return Sieve::new(x as usize).prime_pi(x as usize) as u64;
    }

    let (alpha_y, alpha_z) = get_alpha_gourdon(x);

    let x13 = icbrt(x) as usize;
    let sqrt_x = isqrt(x) as usize;
    let y = std::cmp::max(std::cmp::min((x13 as f64 * alpha_y) as usize, sqrt_x - 1), x13 + 1);
    let z = std::cmp::max(std::cmp::min((y as f64 * alpha_z) as usize, sqrt_x - 1), y);
    let x_star = get_x_star(x, y as u64) as usize;

    let max_a_prime = isqrt(x / std::cmp::max(x_star, 1) as u64) as usize;
    let pi_table_limit = std::cmp::max(z, max_a_prime);
    let show_timing = std::env::var("SHOW_TIMING").is_ok();

    let t_setup = Instant::now();

    // Run ALL setup tasks concurrently:
    // Thread 1: BigPiTable::new (parallel, uses rayon)
    // Thread 2: generate_tables (sequential)
    // Main thread: pi_sieve, primes, phi_cache, generate_pi (sequential)
    let (big_pi, mu, lpf, y_smooth, primes, pi_y, k, phi_cache, pi) = std::thread::scope(|s| {
        let bpi_handle = s.spawn(|| BigPiTable::new(sqrt_x));
        let tables_handle = s.spawn(|| generate_tables(z, y));

        let pi_sieve = Sieve::new(pi_table_limit);
        let mut primes: Vec<u32> = vec![0];
        primes.extend(pi_sieve.primes_from(2).take_while(|&p| p <= y).map(|p| p as u32));
        let pi_y = primes.len() - 1;
        let k = std::cmp::min(7, pi_y);
        let phi_cache = PhiTinyCache::new(k);
        let pi = generate_pi(pi_table_limit, &pi_sieve);

        let big_pi = bpi_handle.join().unwrap();
        let (mu, lpf, y_smooth) = tables_handle.join().unwrap();
        (big_pi, mu, lpf, y_smooth, primes, pi_y, k, phi_cache, pi)
    });
    if show_timing { eprintln!("  setup tables: {:.2}s", t_setup.elapsed().as_secs_f64()); }

    // Sigma and Phi0 (fast, sequential)
    let sigma = compute_sigma(x, y, x_star, &primes, &pi, &big_pi);
    let phi0 = compute_phi0(x, y, z, k, &primes, &phi_cache);

    // Run B, AC, D all concurrently — they share the rayon pool
    let (ac, d, b_val) = std::thread::scope(|s| {
        let b_handle = s.spawn(|| {
            let t = Instant::now();
            let r = compute_b(x, y, pi_y, &big_pi);
            if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
            r
        });
        let ac_handle = s.spawn(|| {
            let t = Instant::now();
            let r = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi);
            if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
            r
        });
        let t = Instant::now();
        let d = compute_d(x, y, z, k, x_star, &primes, &pi, &mu, &lpf, &y_smooth);
        if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
        let ac = ac_handle.join().unwrap();
        let b_val = b_handle.join().unwrap();
        (ac, d, b_val)
    });

    (ac - b_val + d + phi0 + sigma) as u64
}

fn main() {
    // Oversubscribe rayon thread pool for better work-stealing across B/AC/D
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(24);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_cpus * 3)
        .build_global()
        .ok();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V7 — Gourdon's Algorithm                    ║");
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
        Case { limit: 100_000_000_000_000, label: "100 Trillion", expected: 3_204_941_750_802 },
        Case { limit: 1_000_000_000_000_000, label: "1 Quadrillion", expected: 29_844_570_422_669 },
        Case { limit: 10_000_000_000_000_000, label: "10 Quadrillion", expected: 279_238_341_033_925 },
        Case { limit: 100_000_000_000_000_000, label: "100 Quadrillion", expected: 2_623_557_157_654_233 },
        Case { limit: 1_000_000_000_000_000_000, label: "1 Quintillion", expected: 24_739_954_287_740_860 },
        Case { limit: 9_223_372_036_854_775_807, label: "Max i64", expected: 216_289_611_853_439_384 },
    ];

    let limit_filter: Option<u64> = std::env::var("LIMIT").ok()
        .and_then(|v| v.parse().ok());

    println!("{:<15} {:>12} {:>18}  {}", "Range", "Time", "Primes Found", "Status");
    println!("{}", "─".repeat(65));

    for c in &cases {
        if let Some(lf) = limit_filter {
            if c.limit != lf { continue; }
        }
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



















