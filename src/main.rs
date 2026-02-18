use primal::Sieve;
use rayon::prelude::*;
use std::time::Instant;

// ── Wheel mod 30 ─────────────────────────────────────────────────────────────
const OFFSETS: [u32; 8] = [1, 7, 11, 13, 17, 19, 23, 29];
const BITS: [u8; 8] = [1, 2, 4, 8, 16, 32, 64, 128];

// ── Segment sizing ───────────────────────────────────────────────────────────
// Large segments reduce per-prime startup cost but need enough for parallelism.
const MAX_SEG_BYTES: usize = 512 * 1024;
const MIN_SEG_BYTES: usize = 64 * 1024; // Don't go smaller than L1

#[inline(always)]
const fn mod_inv_30(a: u32) -> u32 {
    match a {
        1 => 1, 7 => 13, 11 => 11, 13 => 7,
        17 => 23, 19 => 19, 23 => 17, 29 => 29,
        _ => 1,
    }
}

/// Compute starting byte indices for each wheel residue.
#[inline]
fn compute_starts(p: u64, seg_start: u64, seg_bytes_len: usize) -> [usize; 8] {
    let p_mod = (p % 30) as u32;
    let inv = mod_inv_30(p_mod);
    let seg_end_num = seg_start + (seg_bytes_len as u64) * 30;
    let mut starts = [usize::MAX; 8];

    for (i, &off) in OFFSETS.iter().enumerate() {
        let target_k_mod = ((off as u64) * (inv as u64)) % 30;
        let k_min = (seg_start + p - 1) / p;
        let k_rem = k_min % 30;
        let mut k = if k_rem <= target_k_mod {
            k_min + (target_k_mod - k_rem)
        } else {
            k_min + (30 - k_rem + target_k_mod)
        };
        if k < 2 { k += 30; }

        let m = k * p;
        if m >= seg_end_num { continue; }
        starts[i] = ((m - seg_start) / 30) as usize;
    }
    starts
}

#[inline]
unsafe fn sieve_with_starts(sieve: &mut [u8], starts: &[usize; 8], p: usize) {
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

/// Count prime candidates (zero bits) in the sieve.
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

    let sieving_primes: Vec<u64> = small_sieve
        .primes_from(7)
        .map(|p| p as u64)
        .collect();

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
            for &sp in &sieving_primes {
                if sp * sp > n { break; }
                if n % sp == 0 { is_p = false; break; }
            }
            if is_p { gap_primes += 1; }
        }
    }

    let total_numbers = limit - sieve_start + 1;
    let total_bytes = ((total_numbers + 29) / 30) as usize;

    // Adaptive segment size: ensure enough segments for good work distribution.
    // Want at least 8 segments per thread for effective work-stealing with
    // heterogeneous P+E cores.
    let num_threads = rayon::current_num_threads();
    let min_segments = num_threads * 8;
    let seg_bytes = if total_bytes / MAX_SEG_BYTES >= min_segments {
        MAX_SEG_BYTES
    } else {
        // Shrink to create more segments, but not below MIN_SEG_BYTES
        let ideal = total_bytes / min_segments;
        // Round down to multiple of 8 for alignment
        let aligned = (ideal / 8) * 8;
        aligned.max(MIN_SEG_BYTES).min(MAX_SEG_BYTES)
    };

    let num_segs = (total_bytes + seg_bytes - 1) / seg_bytes;

    let upper_count: u64 = (0..num_segs)
        .into_par_iter()
        .map_init(
            || vec![0u8; seg_bytes],
            |sieve_buf, seg_idx| {
            let byte_offset = seg_idx * seg_bytes;
            let seg_start_num = sieve_start + (byte_offset as u64) * 30;
            let remaining_bytes = total_bytes - byte_offset;
            let seg_byte_count = remaining_bytes.min(seg_bytes);

            // Reuse buffer, zero only the portion we need
            let sieve = &mut sieve_buf[..seg_byte_count];
            sieve.fill(0);

            for &p in &sieving_primes {
                let starts = compute_starts(p, seg_start_num, seg_byte_count);
                unsafe { sieve_with_starts(sieve, &starts, p as usize); }
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
