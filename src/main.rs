use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;

// ── Wheel mod 30 ─────────────────────────────────────────────────────────────
const OFFSETS: [u32; 8] = [1, 7, 11, 13, 17, 19, 23, 29];
const BITS: [u8; 8] = [1, 2, 4, 8, 16, 32, 64, 128];

// ── Segment sizing ───────────────────────────────────────────────────────────
const MAX_SEG_BYTES: usize = 768 * 1024;
const MIN_SEG_BYTES: usize = 16 * 1024;

// ── Pre-sieve pattern ────────────────────────────────────────────────────────
const PRESIEVE_PRIMES: [u64; 3] = [7, 11, 13];
const PRESIEVE_PERIOD: usize = 7 * 11 * 13; // 1001 bytes

#[inline(always)]
const fn mod_inv_30(a: u32) -> u32 {
    match a {
        1 => 1, 7 => 13, 11 => 11, 13 => 7,
        17 => 23, 19 => 19, 23 => 17, 29 => 29,
        _ => 1,
    }
}

const TARGET_K_MOD: [[u64; 8]; 8] = precompute_target_k_mod();

const fn precompute_target_k_mod() -> [[u64; 8]; 8] {
    let mut table = [[0u64; 8]; 8];
    let offsets = [1u64, 7, 11, 13, 17, 19, 23, 29];
    let mut pri = 0;
    while pri < 8 {
        let inv = mod_inv_30(offsets[pri] as u32) as u64;
        let mut oi = 0;
        while oi < 8 {
            table[pri][oi] = (offsets[oi] * inv) % 30;
            oi += 1;
        }
        pri += 1;
    }
    table
}

const P_MOD_TO_IDX: [u8; 30] = {
    let mut t = [0u8; 30];
    t[1] = 0; t[7] = 1; t[11] = 2; t[13] = 3;
    t[17] = 4; t[19] = 5; t[23] = 6; t[29] = 7;
    t
};

/// Precomputed sieving prime with wheel30 metadata and reciprocal for fast division.
struct SievePrime {
    p: u32,
    p_idx: u8,
    // Reciprocal for Barrett reduction: floor(2^64 / p) + 1
    // Allows replacing `n / p` with `mulhi(n, recip) >> shift`
    recip: u64,
}

fn precompute_sieve_primes(primes: &[u32]) -> Vec<SievePrime> {
    primes.iter().map(|&p| {
        let p_idx = P_MOD_TO_IDX[(p % 30) as usize];
        // Barrett reciprocal: ceil(2^64 / p)
        let recip = if p > 1 {
            u64::MAX / (p as u64) + 1
        } else {
            u64::MAX
        };
        SievePrime { p, p_idx, recip }
    }).collect()
}

/// Fast approximate division: floor(n / p) using precomputed reciprocal.
/// Uses the identity: floor(n / p) = mulhi(n, ceil(2^64/p))
/// This is exact for n < 2^63 and p < 2^32.
#[inline(always)]
fn fast_div(n: u64, recip: u64) -> u64 {
    ((n as u128 * recip as u128) >> 64) as u64
}

fn build_presieve() -> Vec<u8> {
    let mut pattern = vec![0u8; PRESIEVE_PERIOD];
    for &p in &PRESIEVE_PRIMES {
        let p_idx = P_MOD_TO_IDX[(p % 30) as usize] as usize;
        for i in 0..8 {
            let target = TARGET_K_MOD[p_idx][i];
            let mut byte_idx = ((target * p) / 30) as usize;
            let step = p as usize;
            while byte_idx < PRESIEVE_PERIOD {
                pattern[byte_idx] |= BITS[i];
                byte_idx += step;
            }
        }
    }
    pattern
}

#[inline]
fn apply_presieve(sieve: &mut [u8], seg_byte_offset: usize, presieve: &[u8]) {
    let offset_in_pattern = seg_byte_offset % PRESIEVE_PERIOD;
    let len = sieve.len();
    let mut dst = 0;

    let first_chunk = (PRESIEVE_PERIOD - offset_in_pattern).min(len);
    sieve[..first_chunk].copy_from_slice(&presieve[offset_in_pattern..offset_in_pattern + first_chunk]);
    dst += first_chunk;

    while dst + PRESIEVE_PERIOD <= len {
        sieve[dst..dst + PRESIEVE_PERIOD].copy_from_slice(presieve);
        dst += PRESIEVE_PERIOD;
    }

    if dst < len {
        let remaining = len - dst;
        sieve[dst..].copy_from_slice(&presieve[..remaining]);
    }
}

/// Compute start byte positions for a sieving prime in a segment.
/// Uses Barrett reciprocal to replace u64 division with multiplication.
#[inline]
fn compute_starts(sp: &SievePrime, seg_start: u64, seg_bytes_len: usize) -> [usize; 8] {
    let p = sp.p as u64;
    let seg_end_num = seg_start + (seg_bytes_len as u64) * 30;
    let mut starts = [usize::MAX; 8];

    // k_min = ceil(seg_start / p) using Barrett reciprocal
    let k_min = fast_div(seg_start + p - 1, sp.recip);
    let k_rem = k_min % 30;

    for i in 0..8 {
        let target = TARGET_K_MOD[sp.p_idx as usize][i];
        let mut k = if k_rem <= target {
            k_min + (target - k_rem)
        } else {
            k_min + (30 - k_rem + target)
        };
        if k < 2 { k += 30; }

        let m = k * p;
        if m >= seg_end_num { continue; }

        let diff = m - seg_start;
        starts[i] = (diff / 30) as usize;
    }
    starts
}

/// Sieve marking for small primes (many hits per segment): 4× unrolled.
#[inline]
unsafe fn sieve_small(sieve: &mut [u8], starts: &[usize; 8], p: usize) {
    let len = sieve.len();

    for i in 0..8 {
        let mut idx = starts[i];
        if idx >= len { continue; }
        let bit = BITS[i];

        while idx + 3 * p < len {
            *sieve.get_unchecked_mut(idx) |= bit;
            *sieve.get_unchecked_mut(idx + p) |= bit;
            *sieve.get_unchecked_mut(idx + 2 * p) |= bit;
            *sieve.get_unchecked_mut(idx + 3 * p) |= bit;
            idx += 4 * p;
        }
        while idx < len {
            *sieve.get_unchecked_mut(idx) |= bit;
            idx += p;
        }
    }
}

/// Sieve marking for large primes (≤1 hit per residue per segment): no unrolling.
#[inline]
unsafe fn sieve_large(sieve: &mut [u8], starts: &[usize; 8]) {
    let len = sieve.len();
    for i in 0..8 {
        let idx = starts[i];
        if idx < len {
            *sieve.get_unchecked_mut(idx) |= BITS[i];
        }
    }
}

#[inline]
fn count_primes_in_sieve(sieve: &[u8], seg_start: u64, limit: u64) -> u64 {
    let valid_bytes = sieve.len();
    if valid_bytes == 0 { return 0; }

    let last_byte_idx = valid_bytes - 1;
    let last_block_start = seg_start + (last_byte_idx as u64) * 30;
    let is_partial = last_block_start + 29 > limit;
    let full_count_bytes = if is_partial { last_byte_idx } else { valid_bytes };

    let mut count = 0u64;
    let chunks = full_count_bytes / 8;
    let ptr = sieve.as_ptr() as *const u64;
    for i in 0..chunks {
        let word = unsafe { ptr.add(i).read_unaligned() };
        count += (64 - word.count_ones()) as u64;
    }
    for i in (chunks * 8)..full_count_bytes {
        count += (8 - sieve[i].count_ones()) as u64;
    }

    if is_partial {
        let byte = sieve[last_byte_idx];
        for (bit_idx, &off) in OFFSETS.iter().enumerate() {
            let n = last_block_start + off as u64;
            if n > limit { continue; }
            if byte & BITS[bit_idx] == 0 {
                count += 1;
            }
        }
    }

    count
}

fn count_primes(limit: u64) -> u64 {
    if limit < 2 { return 0; }
    if limit == 2 { return 1; }
    if limit < 5 { return if limit >= 3 { 2 } else { 1 }; }

    let sqrt_n = ((limit as f64).sqrt() as usize) + 1;
    let small_sieve = Sieve::new(sqrt_n);

    if (limit as usize) <= sqrt_n {
        return small_sieve.prime_pi(limit as usize) as u64;
    }

    let sieving_primes_raw: Vec<u32> = small_sieve
        .primes_from(7)
        .map(|p| p as u32)
        .filter(|&p| !PRESIEVE_PRIMES.contains(&(p as u64)))
        .collect();

    let sieving_primes = precompute_sieve_primes(&sieving_primes_raw);

    let base_prime_count = small_sieve.prime_pi(sqrt_n) as u64;

    let sieve_start: u64 = {
        let s = sqrt_n as u64 + 1;
        ((s + 29) / 30) * 30
    };

    if sieve_start > limit {
        let mut count = base_prime_count;
        for n in (sqrt_n as u64 + 1)..=limit {
            if small_sieve.is_prime(n as usize) { count += 1; }
        }
        return count;
    }

    let mut gap_primes = 0u64;
    for n in (sqrt_n as u64 + 1)..sieve_start {
        let r = (n % 30) as usize;
        if matches!(r, 1|7|11|13|17|19|23|29) {
            let mut is_p = true;
            for sp in &sieving_primes {
                if (sp.p as u64) * (sp.p as u64) > n { break; }
                if n % (sp.p as u64) == 0 { is_p = false; break; }
            }
            if is_p {
                for &pp in &PRESIEVE_PRIMES {
                    if pp * pp > n { break; }
                    if n % pp == 0 { is_p = false; break; }
                }
            }
            if is_p { gap_primes += 1; }
        }
    }

    let presieve = build_presieve();
    let presieve_base_offset = ((sieve_start / 30) as usize) % PRESIEVE_PERIOD;

    let total_numbers = limit - sieve_start + 1;
    let total_bytes = ((total_numbers + 29) / 30) as usize;

    let num_threads = rayon::current_num_threads();
    let min_segments = num_threads * 8;
    let seg_bytes = if total_bytes / MAX_SEG_BYTES >= min_segments {
        MAX_SEG_BYTES
    } else {
        let ideal = total_bytes / min_segments;
        let aligned = (ideal / 8) * 8;
        aligned.max(MIN_SEG_BYTES).min(MAX_SEG_BYTES)
    };

    let num_segs = (total_bytes + seg_bytes - 1) / seg_bytes;

    // Split primes: small primes get unrolled loop, large primes get single-write
    let large_threshold = seg_bytes as u32;
    let split_idx = sieving_primes.partition_point(|sp| sp.p < large_threshold);
    let small_primes = &sieving_primes[..split_idx];
    let large_primes = &sieving_primes[split_idx..];

    let upper_count: u64 = (0..num_segs)
        .into_par_iter()
        .map_init(
            || vec![0u8; seg_bytes],
            |sieve_buf, seg_idx| {
            let byte_offset = seg_idx * seg_bytes;
            let seg_start_num = sieve_start + (byte_offset as u64) * 30;
            let remaining_bytes = total_bytes - byte_offset;
            let seg_byte_count = remaining_bytes.min(seg_bytes);

            let sieve = &mut sieve_buf[..seg_byte_count];

            let pattern_offset = (presieve_base_offset + byte_offset) % PRESIEVE_PERIOD;
            apply_presieve(sieve, pattern_offset, &presieve);

            for sp in small_primes {
                let starts = compute_starts(sp, seg_start_num, seg_byte_count);
                unsafe { sieve_small(sieve, &starts, sp.p as usize); }
            }

            for sp in large_primes {
                let starts = compute_starts(sp, seg_start_num, seg_byte_count);
                unsafe { sieve_large(sieve, &starts); }
            }

            count_primes_in_sieve(sieve, seg_start_num, limit)
        })
        .sum();

    base_prime_count + gap_primes + upper_count
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║   Optimized Segmented Sieve — Wheel30 + Adaptive Segments  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("Threads: {}", rayon::current_num_threads());
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
