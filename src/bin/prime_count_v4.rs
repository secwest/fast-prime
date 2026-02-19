use mimalloc::MiMalloc;
use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

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

// ── P2: Pairs of large primes (parallel sieve) ──────────────────────────────

/// Parallel odd-number sieve with fast π(n) queries.
/// Replaces primal::Sieve for P2 to enable multi-threaded sieve construction.
struct ParallelPiSieve {
    bitmap: Vec<u64>,   // bit i = is_prime(2i+1)
    prefix: Vec<u32>,   // prefix[w] = count of set bits in bitmap[0..w]
}

impl ParallelPiSieve {
    fn new(limit: usize) -> Self {
        if limit < 3 {
            return ParallelPiSieve {
                bitmap: vec![0],
                prefix: vec![0, 0],
            };
        }
        let half = limit / 2 + 1;
        let nwords = (half + 63) / 64;

        let sqrt_limit = isqrt(limit as u64) as usize;
        let small_sieve = Sieve::new(sqrt_limit);
        let cross_primes: Vec<usize> = small_sieve.primes_from(3)
            .take_while(|&p| p <= sqrt_limit)
            .collect();

        let mut bitmap = vec![!0u64; nwords];
        let last_bits = half % 64;
        if last_bits > 0 && nwords > 0 {
            bitmap[nwords - 1] &= (1u64 << last_bits) - 1;
        }

        let chunk_words = std::cmp::max(nwords / rayon::current_num_threads(), 512);
        bitmap.par_chunks_mut(chunk_words)
            .enumerate()
            .for_each(|(chunk_idx, chunk)| {
                let word_start = chunk_idx * chunk_words;
                let bit_start = word_start * 64;
                let chunk_len = chunk.len();
                let num_start = 2 * bit_start + 1;

                if bit_start == 0 && chunk_len > 0 {
                    chunk[0] &= !1u64; // number 1 is not prime
                }

                for &p in &cross_primes {
                    let pp = p * p;
                    let first_num = if pp > num_start {
                        pp
                    } else {
                        let m = ((num_start + p - 1) / p) * p;
                        if m % 2 == 0 { m + p } else { m }
                    };
                    let first_bit = (first_num - 1) / 2;
                    if first_bit >= bit_start + chunk_len * 64 { continue; }
                    let mut idx = if first_bit >= bit_start { first_bit - bit_start } else { continue };
                    let chunk_bits = chunk_len * 64;
                    let ptr = chunk.as_mut_ptr();
                    while idx + p * 3 < chunk_bits {
                        unsafe {
                            let w0 = idx >> 6; let b0 = idx & 63;
                            *ptr.add(w0) &= !(1u64 << b0);
                            let i1 = idx + p; let w1 = i1 >> 6; let b1 = i1 & 63;
                            *ptr.add(w1) &= !(1u64 << b1);
                            let i2 = idx + p * 2; let w2 = i2 >> 6; let b2 = i2 & 63;
                            *ptr.add(w2) &= !(1u64 << b2);
                            let i3 = idx + p * 3; let w3 = i3 >> 6; let b3 = i3 & 63;
                            *ptr.add(w3) &= !(1u64 << b3);
                        }
                        idx += p * 4;
                    }
                    while idx < chunk_bits {
                        unsafe {
                            let w = idx >> 6; let b = idx & 63;
                            *ptr.add(w) &= !(1u64 << b);
                        }
                        idx += p;
                    }
                }
            });

        let mut prefix = vec![0u32; nwords + 1];
        for i in 0..nwords {
            prefix[i + 1] = prefix[i] + bitmap[i].count_ones();
        }

        ParallelPiSieve { bitmap, prefix }
    }

    #[inline]
    fn prime_pi(&self, n: usize) -> usize {
        if n < 2 { return 0; }
        let count_2 = 1usize;
        if n < 3 { return count_2; }
        let largest_odd = if n % 2 == 1 { n } else { n - 1 };
        let bit_idx = (largest_odd - 1) / 2;
        let word = bit_idx / 64;
        let bit = bit_idx % 64;
        let mask = if bit == 63 { !0u64 } else { (1u64 << (bit + 1)) - 1 };
        count_2 + self.prefix[word] as usize + (self.bitmap[word] & mask).count_ones() as usize
    }
}

fn compute_p2(x: u64, y: usize, pi_y: usize) -> i64 {
    let sqrt_x = isqrt(x) as usize;
    if y >= sqrt_x { return 0; }

    let max_val = (x / (y as u64 + 1)) as usize;
    let sieve_limit = std::cmp::max(max_val, sqrt_x);
    let p2_sieve = ParallelPiSieve::new(sieve_limit);

    let pi_sqrt_x = p2_sieve.prime_pi(sqrt_x);

    let small_sieve = Sieve::new(sqrt_x);
    let p2_primes: Vec<usize> = small_sieve.primes_from(y + 1)
        .take_while(|&p| p <= sqrt_x)
        .collect();

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

/// Compute S2 using parallel segmented sieve with phi correction.
/// Each thread processes a chunk of segments with local phi accumulators.
/// Cross-segment phi dependency is resolved via a correction pass.
fn compute_s2(x: u64, y: usize, c: usize, primes: &[u32],
              lpf: &[i32], mu: &[i8], pi: &[u32]) -> i64 {
    let z = (x / y as u64) as usize;
    let sqrt_y = isqrt(y as u64) as usize;
    let pi_sqrty = pi[std::cmp::min(sqrt_y, y)] as usize;
    let pi_y = pi[y] as usize;

    // Adaptive segment size: larger for big z (less overhead), smaller for small z (more parallelism)
    let target_segs = rayon::current_num_threads() * 32;
    let segment_size = std::cmp::max(z / target_segs, 1 << 17).next_power_of_two();

    let template = PreSieveTemplate::new(primes, std::cmp::min(c, primes.len() - 1));

    // Barrett reciprocal table for fast division in easy leaf loop
    let prime_recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    #[inline(always)]
    fn fast_div(n: u64, d: u64, recip_d: u64) -> u64 {
        let q = ((n as u128 * recip_d as u128) >> 64) as u64;
        q + (n - q.wrapping_mul(d) >= d) as u64
    }

    let num_segments = if z == 0 { 1 } else { (z / segment_size) + 1 };

    // For small problems, use serial path
    if num_segments <= 2 {
        return compute_s2_serial(x, y, c, primes, lpf, mu, pi, z, pi_sqrty, pi_y,
                                 segment_size, &template, &prime_recip, fast_div);
    }

    let nprimes = primes.len();

    // Build pi lookup table for segment 0: phi(n,b-1) = pi(n) - b + 2 when primes[b-1]² > n
    let global_pi: Vec<u32> = {
        let n = segment_size;
        let mut is_p = vec![true; n + 1];
        is_p[0] = false;
        if n > 0 { is_p[1] = false; }
        let sqrtn = isqrt(n as u64) as usize;
        for p in 2..=sqrtn {
            if is_p[p] {
                for k in (p * p..=n).step_by(p) { is_p[k] = false; }
            }
        }
        let mut tab = vec![0u32; n + 1];
        for i in 1..=n { tab[i] = tab[i - 1] + is_p[i] as u32; }
        tab
    };

    // Use more chunks than threads for better load balancing via work-stealing.
    // Early segments (near low=0) do 100× more work than late segments.
    let nchunks = std::cmp::min(num_segments, rayon::current_num_threads() * 6);

    // Each chunk returns (s2_local, phi_totals, coefficients, max_b_seen)
    let results: Vec<(i64, Vec<i64>, Vec<i64>, usize)> = (0..nchunks).into_par_iter().map(|tid| {
        let seg_start = tid * num_segments / nchunks;
        let seg_end = (tid + 1) * num_segments / nchunks;

        let mut sieve = BitSieve::new(segment_size);
        let mut phi = vec![0i64; nprimes];
        let mut s2_local = 0i64;
        let mut coeff = vec![0i64; nprimes];
        let mut max_b_seen: usize = 0;

        for seg_idx in seg_start..seg_end {
            let low = seg_idx * segment_size;
            if low > z { break; }
            let high = std::cmp::min(low + segment_size, z + 1);
            let seg_len = high - low;
            let low1 = std::cmp::max(low, 1);

            template.init_sieve(&mut sieve, low, seg_len);

            let max_b = if low1 > 0 {
                pi[std::cmp::min(isqrt(x / low1 as u64) as usize, y)] as usize
            } else { pi_y };
            let max_b = std::cmp::min(max_b, pi_y - 1);
            if max_b > max_b_seen { max_b_seen = max_b; }

            let mut b = c + 1;

            // Hard special leaves
            while b <= std::cmp::min(pi_sqrty, max_b) && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let min_m = std::cmp::max(
                    x_div_prime / high as u64, y as u64 / prime
                ) as usize;
                let max_m = std::cmp::min(
                    x_div_prime / low1 as u64, y as u64
                ) as usize;

                if prime as usize >= max_m { break; }

                let mut prev_pos: Option<usize> = None;
                let mut running_count: i64 = 0;
                for m in (min_m + 1..=max_m).rev() {
                    if mu[m] != 0 && (prime as i32) < lpf[m] {
                        let xpm = (x_div_prime / m as u64) as usize;
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
                            s2_local -= mu[m] as i64 * (phi[b] + count);
                            coeff[b] -= mu[m] as i64;
                            prev_pos = Some(pos);
                        }
                    }
                }

                phi[b] += sieve.count_total();
                let p = prime as usize;
                // Compute starting position from scratch
                let first_mul = ((std::cmp::max(low, p) + p - 1) / p) * p;
                let start = if first_mul >= high { seg_len } else { first_mul - low };
                let mut k = start;
                let bits = sieve.bits.as_mut_ptr();
                let mut delta = 0i64;
                while k + p * 3 < seg_len {
                    unsafe {
                        let w0 = k >> 6; let b0 = k & 63;
                        let old0 = *bits.add(w0);
                        delta += ((old0 >> b0) & 1) as i64;
                        *bits.add(w0) = old0 & !(1u64 << b0);
                        let k1 = k + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                        let old1 = *bits.add(w1);
                        delta += ((old1 >> b1) & 1) as i64;
                        *bits.add(w1) = old1 & !(1u64 << b1);
                        let k2 = k + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                        let old2 = *bits.add(w2);
                        delta += ((old2 >> b2) & 1) as i64;
                        *bits.add(w2) = old2 & !(1u64 << b2);
                        let k3 = k + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                        let old3 = *bits.add(w3);
                        delta += ((old3 >> b3) & 1) as i64;
                        *bits.add(w3) = old3 & !(1u64 << b3);
                    }
                    k += p * 4;
                }
                while k < seg_len {
                    unsafe {
                        let w = k >> 6; let bk = k & 63;
                        let old = *bits.add(w);
                        delta += ((old >> bk) & 1) as i64;
                        *bits.add(w) = old & !(1u64 << bk);
                    }
                    k += p;
                }
                sieve.total -= delta;
                b += 1;
            }

            // Easy special leaves
            // For segment 0: use pi formula phi(n,b-1) = pi(n) - b + 2 when primes[b-1]² ≥ high
            let use_pi_formula = low == 0 && seg_idx == seg_start;
            while b <= max_b && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let l_max = std::cmp::min(
                    (x_div_prime / low1 as u64) as usize, y
                );
                let mut l = pi[std::cmp::min(l_max, y)] as usize;
                let min_m = std::cmp::max(
                    (x_div_prime / high as u64) as usize, prime as usize
                );

                if l < nprimes && prime as usize >= primes[l] as usize { break; }

                if use_pi_formula && b > 1 && (primes[b - 1] as u64) * (primes[b - 1] as u64) >= high as u64 {
                    // Pi-formula fast path: phi(n, b-1) = 1 + max(pi(n) - (b-1), 0)
                    let bm1 = b as i64 - 1;
                    let l_end_bound = if min_m <= y { pi[min_m] as usize } else { pi_y };

                    // When p³ ≥ x, ALL easy leaves have xpq < p ≈ primes[b-1], so phi=1
                    let p_cubed = (prime as u128) * (prime as u128) * (prime as u128);
                    if p_cubed >= x as u128 {
                        let total_leaves = if l > l_end_bound { (l - l_end_bound) as i64 } else { 0 };
                        s2_local += total_leaves;
                        coeff[b] += total_leaves;
                        l = l_end_bound;
                    } else {
                        // Split into phi=1 batch and phi>1 individual lookups
                        let threshold_q = (x_div_prime / primes[b - 1] as u64) as usize;
                        let l_phi1_bound = if threshold_q <= y {
                            pi[threshold_q] as usize
                        } else {
                            pi_y // no phi=1 leaves
                        };
                        let l_batch_stop = std::cmp::max(l_phi1_bound, l_end_bound);
                        if l > l_batch_stop && l_batch_stop > 0 {
                            let batch = (l - l_batch_stop) as i64;
                            s2_local += batch;
                            coeff[b] += batch;
                            l = l_batch_stop;
                        }
                    }
                    // Remaining leaves: pi lookup, bounds guaranteed for segment 0
                    while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                        let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                        let pi_n = unsafe { *global_pi.get_unchecked(xpq) } as i64;
                        s2_local += 1 + std::cmp::max(pi_n - bm1, 0);
                        coeff[b] += 1;
                        l -= 1;
                    }
                } else {
                    let mut prev_pos: Option<usize> = None;
                    let mut running_count: i64 = 0;
                    while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                        let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                        if xpq >= low && xpq < high {
                            let pos = xpq - low;
                            let count = match prev_pos {
                                Some(pp) if pos == pp => running_count,
                                Some(pp) if pos > pp => {
                                    running_count += sieve.count_delta(pp, pos);
                                    running_count
                                }
                                _ => {
                                    running_count = sieve.count(pos);
                                    running_count
                                }
                            };
                            s2_local += phi[b] + count;
                            coeff[b] += 1;
                            prev_pos = Some(pos);
                        }
                        l -= 1;
                    }
                }

                phi[b] += sieve.count_total();
                let p = prime as usize;
                let first_mul = ((std::cmp::max(low, p) + p - 1) / p) * p;
                let start = if first_mul >= high { seg_len } else { first_mul - low };
                let mut k = start;
                let bits = sieve.bits.as_mut_ptr();
                let mut delta = 0i64;
                while k + p * 3 < seg_len {
                    unsafe {
                        let w0 = k >> 6; let b0 = k & 63;
                        let old0 = *bits.add(w0);
                        delta += ((old0 >> b0) & 1) as i64;
                        *bits.add(w0) = old0 & !(1u64 << b0);
                        let k1 = k + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                        let old1 = *bits.add(w1);
                        delta += ((old1 >> b1) & 1) as i64;
                        *bits.add(w1) = old1 & !(1u64 << b1);
                        let k2 = k + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                        let old2 = *bits.add(w2);
                        delta += ((old2 >> b2) & 1) as i64;
                        *bits.add(w2) = old2 & !(1u64 << b2);
                        let k3 = k + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                        let old3 = *bits.add(w3);
                        delta += ((old3 >> b3) & 1) as i64;
                        *bits.add(w3) = old3 & !(1u64 << b3);
                    }
                    k += p * 4;
                }
                while k < seg_len {
                    unsafe {
                        let w = k >> 6; let bk = k & 63;
                        let old = *bits.add(w);
                        delta += ((old >> bk) & 1) as i64;
                        *bits.add(w) = old & !(1u64 << bk);
                    }
                    k += p;
                }
                sieve.total -= delta;
                b += 1;
            }
        }

        (s2_local, phi, coeff, max_b_seen)
    }).collect();

    // Correction pass: fix phi[b] offsets across thread boundaries
    let mut s2 = results[0].0;
    let mut prefix_phi = results[0].1.clone();

    for k in 1..results.len() {
        let (s2_local, ref phi_total, ref coeff, max_b_seen) = results[k];
        // Correction = Σ_b prefix_phi[b] * coeff[b], only to max_b_seen
        let limit = std::cmp::min(max_b_seen + 1, nprimes);
        let mut correction = 0i64;
        for b in 0..limit {
            correction += prefix_phi[b] * coeff[b];
        }
        s2 += s2_local + correction;
        for b in 0..limit {
            prefix_phi[b] += phi_total[b];
        }
    }

    s2
}

/// Serial S2 fallback for small inputs.
fn compute_s2_serial(x: u64, y: usize, c: usize, primes: &[u32],
                     lpf: &[i32], mu: &[i8], pi: &[u32],
                     z: usize, pi_sqrty: usize, pi_y: usize,
                     segment_size: usize, template: &PreSieveTemplate,
                     prime_recip: &[u64],
                     fast_div: fn(u64, u64, u64) -> u64) -> i64 {
    let mut phi: Vec<i64> = vec![0i64; primes.len()];
    let mut next: Vec<usize> = (0..primes.len()).map(|b| primes[b] as usize).collect();
    let mut sieve = BitSieve::new(segment_size);
    let mut s2: i64 = 0;
    let mut low: usize = 0;

    while low <= z {
        let high = std::cmp::min(low + segment_size, z + 1);
        let seg_len = high - low;
        let low1 = std::cmp::max(low, 1);

        template.init_sieve(&mut sieve, low, seg_len);

        let max_b = if low1 > 0 {
            pi[std::cmp::min(isqrt(x / low1 as u64) as usize, y)] as usize
        } else { pi_y };
        let max_b = std::cmp::min(max_b, pi_y - 1);

        let mut b = c + 1;

        while b <= std::cmp::min(pi_sqrty, max_b) && b < primes.len() {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let min_m = std::cmp::max(
                x_div_prime / high as u64, y as u64 / prime
            ) as usize;
            let max_m = std::cmp::min(
                x_div_prime / low1 as u64, y as u64
            ) as usize;

            if prime as usize >= max_m { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            for m in (min_m + 1..=max_m).rev() {
                if mu[m] != 0 && (prime as i32) < lpf[m] {
                    let xpm = (x_div_prime / m as u64) as usize;
                    if xpm >= low && xpm < high {
                        let pos = xpm - low;
                        let count = match prev_pos {
                            None => { running_count = sieve.count(pos); running_count }
                            Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
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
            let bits = sieve.bits.as_mut_ptr();
            let mut delta = 0i64;
            while k + p * 3 < seg_len {
                unsafe {
                    let w0 = k >> 6; let b0 = k & 63;
                    let old0 = *bits.add(w0);
                    delta += ((old0 >> b0) & 1) as i64;
                    *bits.add(w0) = old0 & !(1u64 << b0);
                    let k1 = k + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                    let old1 = *bits.add(w1);
                    delta += ((old1 >> b1) & 1) as i64;
                    *bits.add(w1) = old1 & !(1u64 << b1);
                    let k2 = k + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                    let old2 = *bits.add(w2);
                    delta += ((old2 >> b2) & 1) as i64;
                    *bits.add(w2) = old2 & !(1u64 << b2);
                    let k3 = k + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                    let old3 = *bits.add(w3);
                    delta += ((old3 >> b3) & 1) as i64;
                    *bits.add(w3) = old3 & !(1u64 << b3);
                }
                k += p * 4;
            }
            while k < seg_len {
                unsafe {
                    let w = k >> 6; let bk = k & 63;
                    let old = *bits.add(w);
                    delta += ((old >> bk) & 1) as i64;
                    *bits.add(w) = old & !(1u64 << bk);
                }
                k += p;
            }
            sieve.total -= delta;
            next[b] = low + k;
            b += 1;
        }

        while b <= max_b && b < primes.len() {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let l_max = std::cmp::min(
                (x_div_prime / low1 as u64) as usize, y
            );
            let mut l = pi[std::cmp::min(l_max, y)] as usize;
            let min_m = std::cmp::max(
                (x_div_prime / high as u64) as usize, prime as usize
            );

            if l < primes.len() && prime as usize >= primes[l] as usize { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            while l > 0 && l < primes.len() && (primes[l] as usize) > min_m {
                let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                if xpq >= low && xpq < high {
                    let pos = xpq - low;
                    let count = match prev_pos {
                        Some(pp) if pos == pp => running_count,
                        Some(pp) if pos > pp => {
                            running_count += sieve.count_delta(pp, pos);
                            running_count
                        }
                        _ => { running_count = sieve.count(pos); running_count }
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
            let bits = sieve.bits.as_mut_ptr();
            let mut delta = 0i64;
            while k + p * 3 < seg_len {
                unsafe {
                    let w0 = k >> 6; let b0 = k & 63;
                    let old0 = *bits.add(w0);
                    delta += ((old0 >> b0) & 1) as i64;
                    *bits.add(w0) = old0 & !(1u64 << b0);
                    let k1 = k + p; let w1 = k1 >> 6; let b1 = k1 & 63;
                    let old1 = *bits.add(w1);
                    delta += ((old1 >> b1) & 1) as i64;
                    *bits.add(w1) = old1 & !(1u64 << b1);
                    let k2 = k + p * 2; let w2 = k2 >> 6; let b2 = k2 & 63;
                    let old2 = *bits.add(w2);
                    delta += ((old2 >> b2) & 1) as i64;
                    *bits.add(w2) = old2 & !(1u64 << b2);
                    let k3 = k + p * 3; let w3 = k3 >> 6; let b3 = k3 & 63;
                    let old3 = *bits.add(w3);
                    delta += ((old3 >> b3) & 1) as i64;
                    *bits.add(w3) = old3 & !(1u64 << b3);
                }
                k += p * 4;
            }
            while k < seg_len {
                unsafe {
                    let w = k >> 6; let bk = k & 63;
                    let old = *bits.add(w);
                    delta += ((old >> bk) & 1) as i64;
                    *bits.add(w) = old & !(1u64 << bk);
                }
                k += p;
            }
            sieve.total -= delta;
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

    let alpha = 2.2;
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
