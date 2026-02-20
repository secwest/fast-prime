use mimalloc::MiMalloc;
use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

// ── Deleglise-Rivat prime counting ───────────────────────────────────────────
//
// Formula: π(x) = S1(x,a) + S2(x,a) + π(y) - 1 - P2(x,a)
//   where S2 = S2_trivial + S2_easy + S2_hard
//   y = α · x^{1/3}, a = π(y), z = x/y, c = min(a, 6)
//
// Key innovation vs LMO (V4): "easy" special leaves bypass the sieve entirely,
// using direct π-table lookups. Only "hard" leaves need the segmented sieve.
// This dramatically reduces sieve work for large x.

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

// ── Precomputation tables ────────────────────────────────────────────────────

fn generate_lpf(limit: usize) -> Vec<i32> {
    let mut lpf = vec![i32::MAX; limit + 1];
    lpf[0] = 0;
    for p in 2..=limit {
        if lpf[p] != i32::MAX { continue; }
        for m in (p..=limit).step_by(p) {
            if lpf[m] == i32::MAX { lpf[m] = p as i32; }
        }
    }
    lpf
}

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
        for m in (p2..=limit).step_by(p2) { mu[m] = 0; }
    }
    mu
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

// ── P2: Pairs of large primes (parallel sieve) ──────────────────────────────

struct ParallelPiSieve {
    bitmap: Vec<u64>,
    prefix: Vec<u64>,
}

impl ParallelPiSieve {
    fn new(limit: usize) -> Self {
        if limit < 3 {
            return ParallelPiSieve { bitmap: vec![0], prefix: vec![0, 0] };
        }
        let half = limit / 2 + 1;
        let nwords = (half + 63) / 64;
        let sqrt_limit = isqrt(limit as u64) as usize;
        let small_sieve = Sieve::new(sqrt_limit);
        let cross_primes: Vec<usize> = small_sieve.primes_from(3)
            .take_while(|&p| p <= sqrt_limit).collect();

        let mut bitmap = vec![!0u64; nwords];
        let last_bits = half % 64;
        if last_bits > 0 && nwords > 0 { bitmap[nwords - 1] &= (1u64 << last_bits) - 1; }

        let chunk_words = std::cmp::max(nwords / rayon::current_num_threads(), 512);
        bitmap.par_chunks_mut(chunk_words).enumerate().for_each(|(chunk_idx, chunk)| {
            let word_start = chunk_idx * chunk_words;
            let bit_start = word_start * 64;
            let chunk_len = chunk.len();
            let num_start = 2 * bit_start + 1;
            if bit_start == 0 && chunk_len > 0 { chunk[0] &= !1u64; }
            for &p in &cross_primes {
                let pp = p * p;
                let first_num = if pp > num_start { pp } else {
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
                    unsafe { let w = idx >> 6; let b = idx & 63; *ptr.add(w) &= !(1u64 << b); }
                    idx += p;
                }
            }
        });

        let mut prefix = vec![0u64; nwords + 1];
        for i in 0..nwords { prefix[i + 1] = prefix[i] + bitmap[i].count_ones() as u64; }
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
        .take_while(|&p| p <= sqrt_x).collect();
    let p2: i64 = p2_primes.par_iter()
        .map(|&p| p2_sieve.prime_pi((x / p as u64) as usize) as i64).sum();
    let choose2 = |n: i64| n * (n - 1) / 2;
    p2 - choose2(pi_sqrt_x as i64) + choose2(pi_y as i64)
}

// S2_trivial removed — merged into S2_easy for correctness

// ── S2_easy: easy special leaves via π-table lookup ──────────────────────────
//
// For primes b > π(√y): only type-2 (m = prime) leaves exist.
// Easy leaves have x/(p_b·p_l) ≤ y, and since p_b > √y → p_b² > y ≥ n,
// we can use: φ(n, b-1) = π(n) - b + 2  when n ≥ p_b, or φ = 1 when n < p_b.
//
// Covers ALL easy type-2 leaves for b > π(√y), including b > π(√z) (S2_trivial).

fn compute_s2_easy(x: u64, y: usize, z: usize, c: usize,
                   primes: &[u32], pi: &[u32]) -> i64 {
    let pi_y = pi[y] as usize;
    let sqrt_y = isqrt(y as u64) as usize;
    let pi_sqrty = pi[std::cmp::min(sqrt_y, y)] as usize;

    let min_b = std::cmp::max(c, pi_sqrty) + 1;
    if min_b > pi_y { return 0; }

    // Precompute reciprocals for all primes
    let recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    #[inline(always)]
    fn fast_div_easy(n: u64, d: u64, recip_d: u64) -> u64 {
        let q = ((n as u128 * recip_d as u128) >> 64) as u64;
        q + (n - q.wrapping_mul(d) >= d) as u64
    }

    // Parallel over b
    let sum: i64 = (min_b..=pi_y).into_par_iter().map(|b| {
        let prime = primes[b] as u64;
        let xp = x / prime;

        let z_over_p = (z as u64 / prime) as usize;
        let easy_start_l = pi[std::cmp::min(z_over_p, y)] as usize;
        let min_l = std::cmp::max(easy_start_l, b);

        let xpp = std::cmp::min((xp / prime) as usize, y);
        let nontrivial_end_l = pi[xpp] as usize;

        let mut local_sum: i64 = 0;

        // Trivial easy leaves: φ = 1 each
        let trivial_start = std::cmp::max(nontrivial_end_l, min_l);
        if pi_y > trivial_start {
            local_sum += (pi_y - trivial_start) as i64;
        }

        // Non-trivial easy leaves: φ = π(x/pq) - b + 2
        let nt_max_l = std::cmp::min(nontrivial_end_l, pi_y);
        let mut l = nt_max_l;
        // Prefetch pi-table entries ahead to hide memory latency
        const PREFETCH_DIST: usize = 8;
        for d in 0..std::cmp::min(PREFETCH_DIST, l.saturating_sub(min_l)) {
            let pl = l - d;
            if pl > 0 && pl < primes.len() {
                let pf_xpq = fast_div_easy(xp, primes[pl] as u64, recip[pl]) as usize;
                unsafe { std::arch::x86_64::_mm_prefetch(
                    (pi.as_ptr().add(std::cmp::min(pf_xpq, y))) as *const i8,
                    std::arch::x86_64::_MM_HINT_T0); }
            }
        }
        while l > min_l && l < primes.len() {
            let xpq = fast_div_easy(xp, primes[l] as u64, recip[l]) as usize;
            // Prefetch PREFETCH_DIST iterations ahead
            let pf_l = l.wrapping_sub(PREFETCH_DIST);
            if pf_l > min_l && pf_l < primes.len() {
                let pf_xpq = fast_div_easy(xp, primes[pf_l] as u64, recip[pf_l]) as usize;
                unsafe { std::arch::x86_64::_mm_prefetch(
                    (pi.as_ptr().add(std::cmp::min(pf_xpq, y))) as *const i8,
                    std::arch::x86_64::_MM_HINT_T0); }
            }
            let pi_xpq = pi[std::cmp::min(xpq, y)] as usize;
            let phi_val = pi_xpq as i64 - b as i64 + 2;
            let next_pi = pi_xpq + 1;
            if next_pi < primes.len() && primes[next_pi] as usize <= y {
                let threshold = fast_div_easy(xp, primes[next_pi] as u64, recip[next_pi]) as usize;
                let lmin = std::cmp::max(pi[std::cmp::min(threshold, y)] as usize, min_l);
                if lmin < l {
                    local_sum += phi_val * (l - lmin) as i64;
                    l = lmin;
                    continue;
                }
            }
            local_sum += phi_val;
            l -= 1;
        }

        local_sum
    }).sum();

    sum
}

// ── S2_hard: hard special leaves via segmented sieve ─────────────────────────

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

/// S2_hard: compute special leaves that require sieve computation.
/// Type 1: b ≤ π(√y), all squarefree m — sieve-based
/// Type 2: π(√y) < b ≤ π(√z), hard leaves only (x/(p_b·p_l) ≥ y) — sieve-based
/// Parallelized via delta-phi correction across segment chunks.
fn compute_s2_hard(x: u64, y: usize, z: usize, c: usize,
                   primes: &[u32], lpf: &[i32], mu: &[i8], pi: &[u32]) -> i64 {
    if z == 0 { return 0; }

    let sqrt_y = isqrt(y as u64) as usize;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_sqrty = pi[std::cmp::min(sqrt_y, y)] as usize;
    let pi_sqrtz = pi[std::cmp::min(sqrt_z, y)] as usize;
    let pi_y = pi[y] as usize;

    if c >= pi_sqrtz { return 0; }

    let target_segs = rayon::current_num_threads() * 32;
    let segment_size = std::cmp::max(z / target_segs, 1 << 17).next_power_of_two();
    let template = PreSieveTemplate::new(primes, std::cmp::min(c, primes.len() - 1));

    #[inline(always)]
    fn fast_div(n: u64, d: u64, recip_d: u64) -> u64 {
        let q = ((n as u128 * recip_d as u128) >> 64) as u64;
        q + (n - q.wrapping_mul(d) >= d) as u64
    }

    let prime_recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    let num_segments = if z == 0 { 1 } else { (z / segment_size) + 1 };
    let nprimes = primes.len();
    let max_b_global = pi_sqrtz;

    // For small problems, use serial path
    if num_segments <= 2 {
        return compute_s2_hard_serial(x, y, z, c, primes, lpf, mu, pi,
                                       pi_sqrty, pi_sqrtz, pi_y,
                                       segment_size, &template, &prime_recip, fast_div);
    }

    // Build pi lookup table for segment 0: phi(n,b-1) = 1 + max(pi(n)-(b-1), 0) when primes[b-1]² > n
    // Only worthwhile when segment_size fits in cache; skip for huge segments
    let use_global_pi = segment_size <= (1 << 24); // 16M max
    let global_pi: Vec<u32> = if use_global_pi {
        let n = segment_size;
        let mut is_p = vec![true; n + 1];
        is_p[0] = false;
        if n > 0 { is_p[1] = false; }
        let sqrtn = isqrt(n as u64) as usize;
        for p in 2..=sqrtn {
            if is_p[p] { for k in (p * p..=n).step_by(p) { is_p[k] = false; } }
        }
        let mut tab = vec![0u32; n + 1];
        for i in 1..=n { tab[i] = tab[i - 1] + is_p[i] as u32; }
        tab
    } else {
        vec![]
    };

    let nchunks = std::cmp::min(num_segments, rayon::current_num_threads() * 6);

    let results: Vec<(i64, Vec<i64>, Vec<i64>, usize)> = (0..nchunks).into_par_iter().map(|tid| {
        let seg_start = tid * num_segments / nchunks;
        let seg_end = (tid + 1) * num_segments / nchunks;

        let mut sieve = BitSieve::new(segment_size / 2);
        let mut phi = vec![0i64; nprimes];
        let mut s2_local = 0i64;
        let mut coeff = vec![0i64; nprimes];
        let mut max_b_seen: usize = 0;

        for seg_idx in seg_start..seg_end {
            let low = seg_idx * segment_size;
            if low > z { break; }
            let high = std::cmp::min(low + segment_size, z + 1);
            let odd_seg_len = (high - low) / 2;
            let low1 = std::cmp::max(low, 1);
            if odd_seg_len == 0 { break; }

            template.init_sieve(&mut sieve, low, odd_seg_len);

            let cur_max_b = std::cmp::min(
                pi[std::cmp::min(isqrt(x / low1 as u64) as usize, y)] as usize,
                max_b_global);
            if cur_max_b > max_b_seen { max_b_seen = cur_max_b; }

            let mut b = c + 1;

            // Type 1: b ≤ π(√y), ALL leaves
            while b <= std::cmp::min(pi_sqrty, cur_max_b) && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let min_m = std::cmp::max(
                    x_div_prime / high as u64, y as u64 / prime) as usize;
                let max_m = std::cmp::min(
                    x_div_prime / low1 as u64, y as u64) as usize;

                if prime as usize >= max_m { break; }

                let mut prev_pos: Option<usize> = None;
                let mut running_count: i64 = 0;
                for m in (min_m + 1..=max_m).rev() {
                    if mu[m] != 0 && (prime as i32) < lpf[m] {
                        let xpm = (x_div_prime / m as u64) as usize;
                        if xpm > low && xpm < high {
                            let pos = int_to_odd_bp(xpm, low);
                            let count = match prev_pos {
                                None => { running_count = sieve.count(pos); running_count }
                                Some(pp) if pos == pp => running_count,
                                Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
                            };
                            s2_local -= mu[m] as i64 * (phi[b] + count);
                            coeff[b] -= mu[m] as i64;
                            prev_pos = Some(pos);
                        } else if xpm == low {
                            s2_local -= mu[m] as i64 * phi[b];
                            coeff[b] -= mu[m] as i64;
                        }
                    }
                }

                phi[b] += sieve.count_total();
                let p = prime as usize;
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
                b += 1;
            }

            // Type 2: π(√y) < b ≤ π(√z), HARD leaves only (xpq >= y)
            let sqrt_high = isqrt(high as u64) as usize;
            let use_pi_formula = low == 0 && seg_idx == seg_start;
            while b <= cur_max_b && b < nprimes {
                let prime = primes[b] as u64;
                let x_div_prime = x / prime;
                let l_max = std::cmp::min(
                    (x_div_prime / low1 as u64) as usize, y);
                let mut l = pi[std::cmp::min(l_max, y)] as usize;
                let min_m = std::cmp::max(
                    (x_div_prime / high as u64) as usize, prime as usize);

                if l < nprimes && prime as usize >= primes[l] as usize { break; }

                if use_global_pi && use_pi_formula && b > 1 && (primes[b - 1] as u64) * (primes[b - 1] as u64) >= high as u64 {
                    // Pi-formula fast path: φ(n, b-1) = 1 + max(π(n) - (b-1), 0)
                    let bm1 = b as i64 - 1;
                    // Batch trivial phi=1 leaves: when p³ >= x, all leaves have phi=1
                    let p_cubed = (prime as u128) * (prime as u128) * (prime as u128);
                    let l_end_bound = if min_m <= y { pi[min_m] as usize } else { pi_y };
                    if p_cubed >= x as u128 {
                        // All hard leaves contribute 1 each
                        let mut count = 0i64;
                        let mut ll = l;
                        while ll > l_end_bound && ll > 0 && ll < nprimes && (primes[ll] as usize) > min_m {
                            let xpq = fast_div(x_div_prime, primes[ll] as u64, prime_recip[ll]) as usize;
                            if xpq >= y { count += 1; }
                            ll -= 1;
                        }
                        s2_local += count;
                        coeff[b] += count;
                        l = ll;
                    } else {
                        while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                            let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                            if xpq >= y && xpq < high {
                                let pi_n = unsafe { *global_pi.get_unchecked(xpq) } as i64;
                                s2_local += 1 + std::cmp::max(pi_n - bm1, 0);
                                coeff[b] += 1;
                            }
                            l -= 1;
                        }
                    }
                } else {
                    let mut prev_pos: Option<usize> = None;
                    let mut running_count: i64 = 0;
                    while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                        let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                        if xpq >= y {
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
                                s2_local += phi[b] + count;
                                coeff[b] += 1;
                                prev_pos = Some(pos);
                            } else if xpq == low {
                                s2_local += phi[b];
                                coeff[b] += 1;
                            }
                        }
                        l -= 1;
                    }
                }

                phi[b] += sieve.count_total();
                let p = prime as usize;
                if p > sqrt_high {
                    if p > low && p < high {
                        let pos = int_to_odd_bp(p, low);
                        let w = pos >> 6; let bk = pos & 63;
                        unsafe {
                            let old = *sieve.bits.get_unchecked(w);
                            sieve.total -= ((old >> bk) & 1) as i64;
                            *sieve.bits.get_unchecked_mut(w) = old & !(1u64 << bk);
                        }
                    }
                } else {
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
                b += 1;
            }
        }

        (s2_local, phi, coeff, max_b_seen)
    }).collect();

    // Correction pass: fix phi[b] offsets across chunk boundaries
    let mut s2 = results[0].0;
    let mut prefix_phi = results[0].1.clone();

    for k in 1..results.len() {
        let (s2_local, ref phi_total, ref coeff, max_b_seen) = results[k];
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

/// Serial S2_hard fallback for small inputs.
fn compute_s2_hard_serial(x: u64, y: usize, z: usize, c: usize,
                          primes: &[u32], lpf: &[i32], mu: &[i8], pi: &[u32],
                          pi_sqrty: usize, pi_sqrtz: usize, _pi_y: usize,
                          segment_size: usize, template: &PreSieveTemplate,
                          prime_recip: &[u64],
                          fast_div: fn(u64, u64, u64) -> u64) -> i64 {
    let nprimes = primes.len();
    let mut phi: Vec<i64> = vec![0i64; nprimes];
    let mut next: Vec<usize> = (0..nprimes).map(|b| primes[b] as usize).collect();
    let mut sieve = BitSieve::new(segment_size / 2);
    let mut s2: i64 = 0;
    let mut low: usize = 0;

    while low <= z {
        let high = std::cmp::min(low + segment_size, z + 1);
        let odd_seg_len = (high - low) / 2;
        let low1 = std::cmp::max(low, 1);
        if odd_seg_len == 0 { break; }

        template.init_sieve(&mut sieve, low, odd_seg_len);

        let cur_max_b = std::cmp::min(
            pi[std::cmp::min(isqrt(x / low1 as u64) as usize, y)] as usize,
            pi_sqrtz);

        let mut b = c + 1;

        // Type 1
        while b <= std::cmp::min(pi_sqrty, cur_max_b) && b < nprimes {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let min_m = std::cmp::max(
                x_div_prime / high as u64, y as u64 / prime) as usize;
            let max_m = std::cmp::min(
                x_div_prime / low1 as u64, y as u64) as usize;

            if prime as usize >= max_m { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            for m in (min_m + 1..=max_m).rev() {
                if mu[m] != 0 && (prime as i32) < lpf[m] {
                    let xpm = (x_div_prime / m as u64) as usize;
                    if xpm > low && xpm < high {
                        let pos = int_to_odd_bp(xpm, low);
                        let count = match prev_pos {
                            None => { running_count = sieve.count(pos); running_count }
                            Some(pp) if pos == pp => running_count,
                            Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
                        };
                        s2 -= mu[m] as i64 * (phi[b] + count);
                        prev_pos = Some(pos);
                    } else if xpm == low {
                        s2 -= mu[m] as i64 * phi[b];
                    }
                }
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            let start_int = if next[b] > low { next[b] } else {
                first_odd_multiple(p, low + 1)
            };
            let start = if start_int >= high { odd_seg_len } else { int_to_odd_bp(start_int, low) };
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
            next[b] = low + 1 + 2 * k;
            b += 1;
        }

        // Type 2
        while b <= cur_max_b && b < nprimes {
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let l_max = std::cmp::min(
                (x_div_prime / low1 as u64) as usize, y);
            let mut l = pi[std::cmp::min(l_max, y)] as usize;
            let min_m = std::cmp::max(
                (x_div_prime / high as u64) as usize, prime as usize);

            if l < nprimes && prime as usize >= primes[l] as usize { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                if xpq >= y {
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
                        s2 += phi[b] + count;
                        prev_pos = Some(pos);
                    } else if xpq == low {
                        s2 += phi[b];
                    }
                }
                l -= 1;
            }

            phi[b] += sieve.count_total();
            let p = prime as usize;
            let start_int = if next[b] > low { next[b] } else {
                first_odd_multiple(p, low + 1)
            };
            let start = if start_int >= high { odd_seg_len } else { int_to_odd_bp(start_int, low) };
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
            next[b] = low + 1 + 2 * k;
            b += 1;
        }

        low += segment_size;
    }

    s2
}

// ── Main counting function ───────────────────────────────────────────────────

fn count_primes(x: u64) -> u64 {
    if x < 2 { return 0; }

    // For small x, use primal directly
    if x <= 10_000 {
        return Sieve::new(x as usize).prime_pi(x as usize) as u64;
    }

    // Adaptive alpha — tuned for DR
    let log_x = (x as f64).log10();
    let alpha = if log_x <= 13.0 {
        2.2
    } else if log_x <= 14.0 {
        2.2 + 0.8 * (log_x - 13.0)
    } else if log_x <= 15.0 {
        3.0 + 3.0 * (log_x - 14.0)
    } else if log_x <= 16.0 {
        6.0 + 7.0 * (log_x - 15.0)
    } else {
        13.0 + 3.0 * (log_x - 16.0)
    };
    let y = std::cmp::max((icbrt(x) as f64 * alpha) as usize, 1);
    // Cap y so pi table (4*y bytes) fits in L3 cache (36MB)
    let y = std::cmp::min(y, 9_000_000);
    let z = (x / y as u64) as usize;

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

    // S2 = S2_easy + S2_hard, P2 — all concurrent
    let (s2_easy, s2_hard, p2) = std::thread::scope(|s| {
        let p2_handle = s.spawn(|| compute_p2(x, y, pi_y));
        let s2_easy_handle = s.spawn(|| compute_s2_easy(x, y, z, c, &primes, &pi));

        let s2_hard = compute_s2_hard(x, y, z, c, &primes, &lpf, &mu, &pi);

        let p2 = p2_handle.join().unwrap();
        let s2_easy = s2_easy_handle.join().unwrap();
        (s2_easy, s2_hard, p2)
    });

    let s2 = s2_easy + s2_hard;
    let result = s1 + s2 + pi_y as i64 - 1 - p2;
    result as u64
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V5 — Deleglise-Rivat Method                 ║");
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
