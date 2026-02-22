use mimalloc::MiMalloc;
use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

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

// ── Alpha parameters ─────────────────────────────────────────────────────────

fn get_alpha_gourdon(x: u64) -> (f64, f64) {
    // Allow override via environment
    if let (Ok(ay), Ok(az)) = (std::env::var("ALPHA_Y"), std::env::var("ALPHA_Z")) {
        if let (Ok(ay), Ok(az)) = (ay.parse::<f64>(), az.parse::<f64>()) {
            if ay >= 1.0 && az >= 1.0 { return (ay, az); }
        }
    }

    let logx = (x as f64).ln();

    // Lookup table tuned for Intel Core Ultra 9 285K (24 threads, 36MB L3)
    // (logx, alpha_y, alpha_z)
    const TABLE: &[(f64, f64, f64)] = &[
        (20.0,  2.0, 1.5),   // x ~ 5e8
        (23.0,  3.0, 1.5),   // x ~ 1e10
        (25.3,  4.0, 2.0),   // x ~ 1e11
        (30.0,  6.0, 2.0),   // x ~ 1e13
        (32.2,  6.0, 2.0),   // x ~ 1e14
        (34.5,  7.0, 2.0),   // x ~ 1e15
        (36.8,  8.0, 2.0),   // x ~ 1e16
        (39.1,  8.0, 2.0),   // x ~ 1e17
        (41.4,  9.0, 2.0),   // x ~ 1e18
        (43.6,  9.8, 2.0),   // x ~ Max i64
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
// Memory: ~limit/4 bytes.

struct BigPiTable {
    data: Vec<u64>,     // interleaved: data[2w] = bits (is_prime), data[2w+1] = prefix sum
}

impl BigPiTable {
    fn new(limit: usize) -> Self {
        if limit < 3 {
            return BigPiTable { data: vec![] };
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

        // Assemble bits, build prefix sums, and interleave into data array
        let mut bits = Vec::with_capacity(nwords);
        for seg in &seg_results {
            bits.extend_from_slice(seg);
        }
        bits.truncate(nwords);

        let mut data = vec![0u64; 2 * nwords];
        let mut running = 0u64;
        for i in 0..nwords {
            data[2 * i] = bits[i];         // bits word
            data[2 * i + 1] = running;     // prefix sum (primes in words 0..i-1)
            running += bits[i].count_ones() as u64;
        }

        BigPiTable { data }
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

    #[inline]
    fn pi(&self, n: usize) -> u64 {
        if n < 2 { return 0; }
        let mut count = 1u64; // prime 2
        if n < 3 { return count; }
        let odd_idx = (n - 1) / 2;
        let word = odd_idx / 64;
        let bit = odd_idx % 64;
        let base = word * 2;
        // Both bits and prefix are adjacent in the same cache line
        count += unsafe { *self.data.get_unchecked(base + 1) };
        let mask = if bit == 63 { !0u64 } else { (1u64 << (bit + 1)) - 1 };
        count += (unsafe { *self.data.get_unchecked(base) } & mask).count_ones() as u64;
        count
    }

    #[inline(always)]
    fn bits_word(&self, w: usize) -> u64 {
        unsafe { *self.data.get_unchecked(w * 2) }
    }

    #[inline(always)]
    #[cfg(target_arch = "x86_64")]
    fn prefetch(&self, n: usize) {
        if n >= 3 {
            let base = (n - 1) / 2 / 64 * 2;
            unsafe {
                // bits and prefix in same cache line — single prefetch suffices
                _mm_prefetch(self.data.as_ptr().add(base) as *const i8, _MM_HINT_T0);
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
// Uses BigPiTable bits to iterate primes (eliminates redundant Sieve::new(√x)).
// Pre-sieves 3,5,7,11,13 for faster segmented sieve over [0, x/y].

fn compute_b(x: u64, y: usize, _pi_y: usize, big_pi: &BigPiTable) -> i64 {
    let sqrt_x = isqrt(x) as usize;
    if y >= sqrt_x { return 0; }

    // Iterate BigPiTable bits in reverse to collect x/p values ascending
    let bp_start = (y + 1) / 2;  // first odd-index > y
    let bp_end = (sqrt_x - 1) / 2;
    if bp_start > bp_end { return 0; }
    let bp_sw = bp_start / 64;
    let bp_ew = bp_end / 64;

    let mut xp_odd_asc: Vec<usize> = Vec::new();
    let mut max_xp: usize = 0;
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
        // Extract bits high-to-low for descending p (ascending x/p)
        while w != 0 {
            let bit = 63 - w.leading_zeros() as usize;
            w ^= 1u64 << bit;
            let p = (2 * (word_idx * 64 + bit) + 1) as u64;
            let xp = (x / p) as usize;
            if xp > max_xp { max_xp = xp; }
            xp_odd_asc.push(xp.saturating_sub(1) / 2);
        }
    }

    if xp_odd_asc.is_empty() || max_xp < 2 { return 0; }

    // Sieving primes ≥ 17 from BigPiTable (3,5,7,11,13 pre-sieved)
    let sqrt_maxp = isqrt(max_xp as u64) as usize;
    let mut sieve_primes: Vec<usize> = Vec::new();
    if sqrt_maxp >= 17 {
        let sp_s = (17 - 1) / 2; // odd-index of 17
        let sp_e = (sqrt_maxp - 1) / 2;
        for word_idx in (sp_s / 64)..=(sp_e / 64) {
            let mut w = big_pi.bits_word(word_idx);
            if word_idx == sp_s / 64 {
                let sb = sp_s % 64;
                if sb > 0 { w &= !((1u64 << sb) - 1); }
            }
            if word_idx == sp_e / 64 {
                let eb = sp_e % 64;
                if eb < 63 { w &= (1u64 << (eb + 1)) - 1; }
            }
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                w &= w - 1;
                sieve_primes.push(2 * (word_idx * 64 + bit) + 1);
            }
        }
    }

    let masks3 = BigPiTable::build_presieve_masks(3);
    let masks5 = BigPiTable::build_presieve_masks(5);
    let masks7 = BigPiTable::build_presieve_masks(7);
    let masks11 = BigPiTable::build_presieve_masks(11);
    let masks13 = BigPiTable::build_presieve_masks(13);

    let odd_count = (max_xp - 1) / 2 + 1;
    let seg_words: usize = 4096; // 32KB = fits L1
    let seg_odd_count = seg_words * 64;
    let num_segs = (odd_count + seg_odd_count - 1) / seg_odd_count;

    let nthreads = rayon::current_num_threads();
    let nchunks = std::cmp::min(nthreads * 4, num_segs).max(1);
    let chunk_segs = (num_segs + nchunks - 1) / nchunks;

    let chunk_xp_bounds: Vec<usize> = (0..=nchunks).map(|k| {
        let chunk_odd_start = std::cmp::min(
            k as u64 * chunk_segs as u64 * seg_odd_count as u64,
            odd_count as u64) as usize;
        xp_odd_asc.partition_point(|&v| v < chunk_odd_start)
    }).collect();

    let chunk_results: Vec<(i64, u64, usize)> = (0..nchunks).into_par_iter().map(|k| {
        let seg_lo = k * chunk_segs;
        let seg_hi = std::cmp::min(seg_lo + chunk_segs, num_segs);
        let xp_lo = chunk_xp_bounds[k];
        let xp_hi = chunk_xp_bounds[k + 1];

        let mut local_sum: i64 = 0;
        let mut local_pi: u64 = 0;
        let mut xp_idx = xp_lo;
        let mut buf = vec![0u64; seg_words];
        let mut seg_prefix = vec![0u64; seg_words + 1];

        for seg in seg_lo..seg_hi {
            let start_idx = seg * seg_odd_count;
            let end_idx = std::cmp::min(start_idx + seg_odd_count, odd_count);
            let seg_len = end_idx - start_idx;
            let seg_nw = (seg_len + 63) / 64;

            for w in 0..seg_nw { unsafe { *buf.get_unchecked_mut(w) = !0u64; } }
            let excess = seg_nw * 64 - seg_len;
            if excess > 0 { buf[seg_nw - 1] &= u64::MAX >> excess; }
            if seg == 0 { buf[0] &= !1u64; }

            // Pre-sieve 3, 5, 7, 11, 13
            let mut o3 = (1 + 3 - start_idx % 3) % 3;
            let mut o5 = (2 + 5 - start_idx % 5) % 5;
            let mut o7 = (3 + 7 - start_idx % 7) % 7;
            let mut o11 = (5 + 11 - start_idx % 11) % 11;
            let mut o13 = (6 + 13 - start_idx % 13) % 13;
            for w in 0..seg_nw {
                unsafe {
                    *buf.get_unchecked_mut(w) &=
                        masks3[o3] & masks5[o5] & masks7[o7]
                        & masks11[o11] & masks13[o13];
                }
                o3 = (o3 + 2) % 3;
                o5 = (o5 + 1) % 5;
                o7 = (o7 + 6) % 7;
                o11 = (o11 + 2) % 11;
                o13 = (o13 + 1) % 13;
            }
            if seg == 0 {
                buf[0] |= (1u64 << 1) | (1u64 << 2) | (1u64 << 3)
                        | (1u64 << 5) | (1u64 << 6); // restore 3,5,7,11,13
            }

            // Cross off composites for primes ≥ 17
            let end_num = 2 * (end_idx - 1) + 1;
            let max_p = isqrt(end_num as u64) as usize;
            let sp_end = sieve_primes.partition_point(|&p| p <= max_p);
            let low_num = 2 * start_idx + 1;
            for &p in &sieve_primes[..sp_end] {
                let pp = (p as u64) * (p as u64);
                let first_num = if pp >= low_num as u64 {
                    pp as usize
                } else {
                    let m = ((low_num + p - 1) / p) * p;
                    if m % 2 == 0 { m + p } else { m }
                };
                if first_num > end_num { continue; }
                let local_idx = (first_num - 1) / 2 - start_idx;
                let mut idx = local_idx;
                while idx < seg_len {
                    unsafe {
                        let w = idx >> 6;
                        let b = idx & 63;
                        *buf.get_unchecked_mut(w) &= !(1u64 << b);
                    }
                    idx += p;
                }
            }

            seg_prefix[0] = 0;
            for w in 0..seg_nw {
                seg_prefix[w + 1] = seg_prefix[w]
                    + unsafe { *buf.get_unchecked(w) }.count_ones() as u64;
            }
            let seg_prime_count = seg_prefix[seg_nw];

            while xp_idx < xp_hi {
                let oi = unsafe { *xp_odd_asc.get_unchecked(xp_idx) };
                if oi >= end_idx { break; }
                let pos = oi - start_idx;
                let word = pos >> 6;
                let bit = pos & 63;
                let mask = if bit == 63 { !0u64 } else { (1u64 << (bit + 1)) - 1 };
                let count = unsafe { *seg_prefix.get_unchecked(word) }
                    + (unsafe { *buf.get_unchecked(word) } & mask).count_ones() as u64;
                local_sum += (1 + local_pi + count) as i64;
                xp_idx += 1;
            }

            local_pi += seg_prime_count;
        }

        (local_sum, local_pi, xp_idx - xp_lo)
    }).collect();

    let mut total_sum: i64 = 0;
    let mut prefix_pi: u64 = 0;
    for &(local_sum, local_pi, num_lookups) in &chunk_results {
        total_sum += local_sum + prefix_pi as i64 * num_lookups as i64;
        prefix_pi += local_pi;
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

    // ── C2 + A: parallel over b values ───────────────────────────────────
    // Precompute reciprocals
    let recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    // C2: max(k, pi_root3_xy, pi_sqrtz) < b ≤ π(x*)
    let min_c2_b = std::cmp::max(k, std::cmp::max(pi_root3_xy, pi_sqrtz)) + 1;
    let c2: i64 = if min_c2_b <= pi_x_star {
        (min_c2_b..=pi_x_star).into_par_iter().map(|b| {
            if b >= primes.len() { return 0i64; }
            let prime = primes[b] as u64;
            let xp = x / prime;
            let max_m = std::cmp::min(std::cmp::min((xp / prime) as usize, y),
                                       (xp / std::cmp::max(1, prime) as u64) as usize);
            let min_m_val = std::cmp::max(
                std::cmp::max((xp / (prime * prime)) as usize, prime as usize),
                1);
            let min_m_val = std::cmp::min(min_m_val, max_m);

            if max_m <= min_m_val { return 0i64; }

            let mut l = pi[std::cmp::min(max_m, pi_limit)] as usize;
            let pi_min = pi[std::cmp::min(min_m_val, pi_limit)] as usize;
            let mut local = 0i64;

            // Clustered optimization: batch consecutive l with same pi(x/pq)
            let sqrt_xp = isqrt(xp) as usize;
            let min_clustered = std::cmp::max(min_m_val, std::cmp::min(sqrt_xp, max_m));
            let pi_min_clustered = pi[std::cmp::min(min_clustered, pi_limit)] as usize;

            // Clustered region
            while l > pi_min_clustered && l > pi_min && l < primes.len() {
                let xpq = fast_div(xp, primes[l] as u64, recip[l]) as usize;
                let pi_xpq = big_pi.pi(std::cmp::min(xpq, sqrt_x)) as i64;
                let phi_val = pi_xpq - b as i64 + 2;
                if (pi_xpq as usize + 1) < primes.len() {
                    let next_p = primes[pi_xpq as usize + 1] as u64;
                    let xpq2 = fast_div(xp, next_p, recip[pi_xpq as usize + 1]) as usize;
                    let l_min = std::cmp::max(pi[std::cmp::min(xpq2, pi_limit)] as usize, pi_min);
                    if l_min < l {
                        local += phi_val * (l - l_min) as i64;
                        l = l_min;
                        continue;
                    }
                }
                local += phi_val;
                l -= 1;
            }

            // Sparse region (4x unrolled with prefetch)
            while l > pi_min + 3 && l + 3 < primes.len() {
                // Prefetch next iteration's 4 BigPiTable lookups
                if l > pi_min + 7 {
                    let pf0 = fast_div(xp, primes[l-4] as u64, recip[l-4]) as usize;
                    let pf1 = fast_div(xp, primes[l-5] as u64, recip[l-5]) as usize;
                    let pf2 = fast_div(xp, primes[l-6] as u64, recip[l-6]) as usize;
                    let pf3 = fast_div(xp, primes[l-7] as u64, recip[l-7]) as usize;
                    big_pi.prefetch(std::cmp::min(pf0, sqrt_x));
                    big_pi.prefetch(std::cmp::min(pf1, sqrt_x));
                    big_pi.prefetch(std::cmp::min(pf2, sqrt_x));
                    big_pi.prefetch(std::cmp::min(pf3, sqrt_x));
                }
                let xpq0 = fast_div(xp, primes[l] as u64, recip[l]) as usize;
                let xpq1 = fast_div(xp, primes[l-1] as u64, recip[l-1]) as usize;
                let xpq2 = fast_div(xp, primes[l-2] as u64, recip[l-2]) as usize;
                let xpq3 = fast_div(xp, primes[l-3] as u64, recip[l-3]) as usize;
                local += (big_pi.pi(std::cmp::min(xpq0, sqrt_x)) as i64 - b as i64 + 2)
                       + (big_pi.pi(std::cmp::min(xpq1, sqrt_x)) as i64 - b as i64 + 2)
                       + (big_pi.pi(std::cmp::min(xpq2, sqrt_x)) as i64 - b as i64 + 2)
                       + (big_pi.pi(std::cmp::min(xpq3, sqrt_x)) as i64 - b as i64 + 2);
                l -= 4;
            }

            while l > pi_min && l < primes.len() {
                let xpq = fast_div(xp, primes[l] as u64, recip[l]) as usize;
                local += big_pi.pi(std::cmp::min(xpq, sqrt_x)) as i64 - b as i64 + 2;
                l -= 1;
            }

            local
        }).sum()
    } else { 0 };

    // A: π(x*) < b ≤ π(x^{1/3})
    let a_sum: i64 = if pi_x_star < pi_x13 {
        ((pi_x_star + 1)..=pi_x13).into_par_iter().map(|b| {
            if b >= primes.len() { return 0i64; }
            let prime = primes[b] as u64;
            let xp = x / prime;
            let sqrt_xp = isqrt(xp) as usize;
            let max_2nd = std::cmp::min(sqrt_xp, y);
            let min_2nd = std::cmp::max(prime as usize, 1);

            if max_2nd <= min_2nd { return 0i64; }

            let max_i = pi[std::cmp::min(max_2nd, pi_limit)] as usize;
            let min_i = pi[std::cmp::min(min_2nd, pi_limit)] as usize + 1;

            if min_i > max_i { return 0i64; }

            // Split at y boundary: x/pq >= y vs x/pq < y
            let xp_over_y = (xp / y as u64) as usize;
            let max_i1 = pi[std::cmp::min(std::cmp::min(xp_over_y, max_2nd), pi_limit)] as usize;

            let mut local = 0i64;

            // x/pq >= y: sum += π(x/pq)
            {
                let mut i = min_i;
                while i + 3 <= max_i1 && i + 3 < primes.len() {
                    // Prefetch next 4
                    if i + 7 <= max_i1 && i + 7 < primes.len() {
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+4] as u64, recip[i+4]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+5] as u64, recip[i+5]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+6] as u64, recip[i+6]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+7] as u64, recip[i+7]) as usize, sqrt_x));
                    }
                    local += big_pi.pi(std::cmp::min(fast_div(xp, primes[i] as u64, recip[i]) as usize, sqrt_x)) as i64
                           + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+1] as u64, recip[i+1]) as usize, sqrt_x)) as i64
                           + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+2] as u64, recip[i+2]) as usize, sqrt_x)) as i64
                           + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+3] as u64, recip[i+3]) as usize, sqrt_x)) as i64;
                    i += 4;
                }
                while i <= max_i1 && i < primes.len() {
                    let xpq = fast_div(xp, primes[i] as u64, recip[i]) as usize;
                    local += big_pi.pi(std::cmp::min(xpq, sqrt_x)) as i64;
                    i += 1;
                }
            }

            // x/pq < y: sum += 2 · π(x/pq)
            let i_start = std::cmp::max(max_i1 + 1, min_i);
            {
                let mut i = i_start;
                while i + 3 <= max_i && i + 3 < primes.len() {
                    if i + 7 <= max_i && i + 7 < primes.len() {
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+4] as u64, recip[i+4]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+5] as u64, recip[i+5]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+6] as u64, recip[i+6]) as usize, sqrt_x));
                        big_pi.prefetch(std::cmp::min(fast_div(xp, primes[i+7] as u64, recip[i+7]) as usize, sqrt_x));
                    }
                    local += 2 * (big_pi.pi(std::cmp::min(fast_div(xp, primes[i] as u64, recip[i]) as usize, sqrt_x)) as i64
                                + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+1] as u64, recip[i+1]) as usize, sqrt_x)) as i64
                                + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+2] as u64, recip[i+2]) as usize, sqrt_x)) as i64
                                + big_pi.pi(std::cmp::min(fast_div(xp, primes[i+3] as u64, recip[i+3]) as usize, sqrt_x)) as i64);
                    i += 4;
                }
                while i <= max_i && i < primes.len() {
                    let xpq = fast_div(xp, primes[i] as u64, recip[i]) as usize;
                    local += 2 * big_pi.pi(std::cmp::min(xpq, sqrt_x)) as i64;
                    i += 1;
                }
            }

            local
        }).sum()
    } else { 0 };

    c1 + c2 + a_sum
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
    period: usize,
}

impl PreSieveTemplate {
    fn new(primes: &[u32], c: usize) -> Self {
        let mut period: usize = 1;
        for b in 2..=c { period *= primes[b] as usize; }
        let mut tpl = vec![true; period];
        for b in 2..=c {
            let p = primes[b] as usize;
            let mut k = (p - 1) / 2;
            while k < period { tpl[k] = false; k += p; }
        }
        let double_period = period * 2;
        let nwords = (double_period + 63) / 64;
        let mut bits = vec![0u64; nwords];
        for i in 0..double_period {
            if tpl[i % period] { bits[i / 64] |= 1u64 << (i % 64); }
        }
        PreSieveTemplate { bits, period }
    }

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

    fn init_sieve(&self, sieve: &mut BitSieve, low: usize, odd_seg_len: usize) {
        sieve.len = odd_seg_len;
        let nwords = (odd_seg_len + 63) / 64;
        let mut tpl_pos = (low / 2) % self.period;
        let mut total = 0i64;
        for w in 0..nwords {
            let word = self.get_word(tpl_pos);
            unsafe { *sieve.bits.get_unchecked_mut(w) = word; }
            total += word.count_ones() as i64;
            tpl_pos += 64;
            if tpl_pos >= self.period { tpl_pos -= self.period; }
        }
        let excess = nwords * 64 - odd_seg_len;
        if excess > 0 {
            let last = unsafe { *sieve.bits.get_unchecked(nwords - 1) };
            let masked = last & (u64::MAX >> excess);
            total -= (last.count_ones() as i64) - (masked.count_ones() as i64);
            unsafe { *sieve.bits.get_unchecked_mut(nwords - 1) = masked; }
        }
        sieve.total = total;
    }
}

#[inline(always)]
fn int_to_odd_bp(n: usize, low: usize) -> usize { (n - low - 1) / 2 }

#[inline(always)]
fn first_odd_multiple(p: usize, low_bound: usize) -> usize {
    let m = ((low_bound + p - 1) / p) * p;
    if m % 2 == 0 { m + p } else { m }
}

fn cross_off_sieve(sieve: &mut BitSieve, prime: usize, low: usize, high: usize,
                   odd_seg_len: usize) {
    let p = prime;
    // For the phi sieve we must cross off ALL odd multiples of p (not just ≥ p²)
    let fom = first_odd_multiple(p, std::cmp::max(low + 1, p));
    let start = if fom >= high { odd_seg_len } else { int_to_odd_bp(fom, low) };
    let mut k = start;
    let bits = sieve.bits.as_mut_ptr();
    let mut delta = 0i64;
    while k + p * 3 < odd_seg_len {
        unsafe {
            let w0 = k >> 6; let b0 = k & 63;
            let old0 = *bits.add(w0); delta += ((old0 >> b0) & 1) as i64;
            *bits.add(w0) = old0 & !(1u64 << b0);
            let k1 = k + p; let w1 = k1 >> 6; let b1 = k1 & 63;
            let old1 = *bits.add(w1); delta += ((old1 >> b1) & 1) as i64;
            *bits.add(w1) = old1 & !(1u64 << b1);
            let k2 = k + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
            let old2 = *bits.add(w2); delta += ((old2 >> b2) & 1) as i64;
            *bits.add(w2) = old2 & !(1u64 << b2);
            let k3 = k + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
            let old3 = *bits.add(w3); delta += ((old3 >> b3) & 1) as i64;
            *bits.add(w3) = old3 & !(1u64 << b3);
        }
        k += p * 4;
    }
    while k < odd_seg_len {
        unsafe {
            let w = k >> 6; let bk = k & 63;
            let old = *bits.add(w); delta += ((old >> bk) & 1) as i64;
            *bits.add(w) = old & !(1u64 << bk);
        }
        k += p;
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
            }).collect()
    } else { vec![] };

    let target_segs = rayon::current_num_threads() * 32;
    let seg_cap = std::env::var("D_SEG_CAP").ok()
        .and_then(|s| s.parse::<u32>().ok()).unwrap_or(21);
    let segment_size = std::cmp::max(
        std::cmp::min(xz / std::cmp::max(target_segs, 1), 1usize << seg_cap),
        1 << 17
    ).next_power_of_two();
    let num_segments = if xz == 0 { 1 } else { (xz / segment_size) + 1 };

    if num_segments <= 2 {
        return compute_d_serial(x, y, z, k, x_star, xz, primes, pi,
                                mu, lpf, y_smooth, pi_sqrtz, pi_x_star,
                                segment_size, &template, &prime_recip,
                                &valid_m_list);
    }

    let nchunks = std::cmp::min(num_segments, rayon::current_num_threads() * 6);

    let results: Vec<(i64, Vec<i64>, Vec<i64>, usize)> = (0..nchunks).into_par_iter().map(|tid| {
        let seg_start = tid * num_segments / nchunks;
        let seg_end = (tid + 1) * num_segments / nchunks;

        let mut sieve = BitSieve::new(segment_size / 2);
        let mut phi = vec![0i64; nprimes];
        let mut d_local = 0i64;
        let mut coeff = vec![0i64; nprimes];
        let mut max_b_seen: usize = 0;

        for seg_idx in seg_start..seg_end {
            let low = seg_idx * segment_size;
            if low > xz { break; }
            let high = std::cmp::min(low + segment_size, xz + 1);
            let odd_seg_len = (high - low) / 2;
            let low1 = std::cmp::max(low, 1);
            if odd_seg_len == 0 { break; }

            template.init_sieve(&mut sieve, low, odd_seg_len);

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
                    let vm_start = valid_m_list.partition_point(|v| (v.m as usize) <= min_m);
                    let vm_end = valid_m_list.partition_point(|v| (v.m as usize) <= max_m);

                    let mut prev_pos: Option<usize> = None;
                    let mut running_count: i64 = 0;
                    for v in valid_m_list[vm_start..vm_end].iter().rev() {
                        if prime < v.lpf as u64 {
                            let xpm = (x_div_prime / v.m as u64) as usize;
                            if xpm > low && xpm < high {
                                let pos = int_to_odd_bp(xpm, low);
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
                cross_off_sieve(&mut sieve, prime as usize, low, high, odd_seg_len);
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
                        let pos = int_to_odd_bp(xpq, low);
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
                    }
                    l -= 1;
                }

                phi[b] += sieve.count_total();
                cross_off_sieve(&mut sieve, prime as usize, low, high, odd_seg_len);
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
    let mut next: Vec<usize> = (0..nprimes).map(|b| primes[b] as usize).collect();
    let mut sieve = BitSieve::new(segment_size / 2);
    let mut d: i64 = 0;
    let mut low: usize = 0;


    while low <= xz {
        let high = std::cmp::min(low + segment_size, xz + 1);
        let odd_seg_len = (high - low) / 2;
        let low1 = std::cmp::max(low, 1);
        if odd_seg_len == 0 { break; }

        template.init_sieve(&mut sieve, low, odd_seg_len);

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
                        let xpm = (x_div_prime / v.m as u64) as usize;
                        if xpm > low && xpm < high {
                            let pos = int_to_odd_bp(xpm, low);
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
            let start_int = if next[b] > low { next[b] } else {
                first_odd_multiple(p, low + 1)
            };
            let start = if start_int >= high { odd_seg_len } else { int_to_odd_bp(start_int, low) };
            let mut kk = start;
            let bits = sieve.bits.as_mut_ptr();
            let mut delta = 0i64;
            while kk + p * 3 < odd_seg_len {
                unsafe {
                    let w0 = kk >> 6; let b0 = kk & 63;
                    let old0 = *bits.add(w0); delta += ((old0 >> b0) & 1) as i64;
                    *bits.add(w0) = old0 & !(1u64 << b0);
                    let k1 = kk + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                    let old1 = *bits.add(w1); delta += ((old1 >> b1) & 1) as i64;
                    *bits.add(w1) = old1 & !(1u64 << b1);
                    let k2 = kk + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                    let old2 = *bits.add(w2); delta += ((old2 >> b2) & 1) as i64;
                    *bits.add(w2) = old2 & !(1u64 << b2);
                    let k3 = kk + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                    let old3 = *bits.add(w3); delta += ((old3 >> b3) & 1) as i64;
                    *bits.add(w3) = old3 & !(1u64 << b3);
                }
                kk += p * 4;
            }
            while kk < odd_seg_len {
                unsafe {
                    let w = kk >> 6; let bk = kk & 63;
                    let old = *bits.add(w); delta += ((old >> bk) & 1) as i64;
                    *bits.add(w) = old & !(1u64 << bk);
                }
                kk += p;
            }
            sieve.total -= delta;
            next[b] = low + 1 + 2 * kk;
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
                    let pos = int_to_odd_bp(xpq, low);
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
                }
                l -= 1;
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            let start_int = if next[b] > low { next[b] } else {
                first_odd_multiple(p, low + 1)
            };
            let start = if start_int >= high { odd_seg_len } else { int_to_odd_bp(start_int, low) };
            let mut kk = start;
            let bits = sieve.bits.as_mut_ptr();
            let mut delta = 0i64;
            while kk + p * 3 < odd_seg_len {
                unsafe {
                    let w0 = kk >> 6; let b0 = kk & 63;
                    let old0 = *bits.add(w0); delta += ((old0 >> b0) & 1) as i64;
                    *bits.add(w0) = old0 & !(1u64 << b0);
                    let k1 = kk + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                    let old1 = *bits.add(w1); delta += ((old1 >> b1) & 1) as i64;
                    *bits.add(w1) = old1 & !(1u64 << b1);
                    let k2 = kk + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                    let old2 = *bits.add(w2); delta += ((old2 >> b2) & 1) as i64;
                    *bits.add(w2) = old2 & !(1u64 << b2);
                    let k3 = kk + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                    let old3 = *bits.add(w3); delta += ((old3 >> b3) & 1) as i64;
                    *bits.add(w3) = old3 & !(1u64 << b3);
                }
                kk += p * 4;
            }
            while kk < odd_seg_len {
                unsafe {
                    let w = kk >> 6; let bk = kk & 63;
                    let old = *bits.add(w); delta += ((old >> bk) & 1) as i64;
                    *bits.add(w) = old & !(1u64 << bk);
                }
                kk += p;
            }
            sieve.total -= delta;
            next[b] = low + 1 + 2 * kk;
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
    let pi_sieve = Sieve::new(pi_table_limit);
    let mut primes: Vec<u32> = vec![0];
    primes.extend(pi_sieve.primes_from(2).take_while(|&p| p <= y).map(|p| p as u32));
    let pi_y = primes.len() - 1;
    let k = std::cmp::min(6, pi_y);
    let phi_cache = PhiTinyCache::new(k);
    let pi = generate_pi(pi_table_limit, &pi_sieve);

    // Overlap generate_tables with BigPiTable construction
    let (big_pi, mu, lpf, y_smooth) = std::thread::scope(|s| {
        let bpi_handle = s.spawn(|| BigPiTable::new(sqrt_x));
        let (mu, lpf, y_smooth) = generate_tables(z, y);
        let big_pi = bpi_handle.join().unwrap();
        (big_pi, mu, lpf, y_smooth)
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
