use mimalloc::MiMalloc;
use primal::Sieve;
use rayon::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[derive(Copy, Clone)]
struct RuntimeTuning {
    ac_seg: usize,
    ac_par_min: usize,
    b_chunks: usize,
    d_chunks: usize,
    d_adapt_chunks: bool,
}

static RUNTIME_TUNING: OnceLock<Mutex<Option<RuntimeTuning>>> = OnceLock::new();
static VM_STRIDE_TUNING: OnceLock<usize> = OnceLock::new();
static VM_LOOKAHEAD_TUNING: OnceLock<usize> = OnceLock::new();

fn tuning_slot() -> &'static Mutex<Option<RuntimeTuning>> {
    RUNTIME_TUNING.get_or_init(|| Mutex::new(None))
}

fn get_runtime_tuning() -> Option<RuntimeTuning> {
    tuning_slot().lock().ok().and_then(|g| *g)
}

fn set_runtime_tuning(tuning: Option<RuntimeTuning>) {
    if let Ok(mut g) = tuning_slot().lock() {
        *g = tuning;
    }
}

fn get_vm_stride() -> usize {
    *VM_STRIDE_TUNING.get_or_init(|| {
        std::env::var("VM_STRIDE").ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(16)
    })
}

fn get_vm_lookahead() -> usize {
    *VM_LOOKAHEAD_TUNING.get_or_init(|| {
        std::env::var("VM_LOOKAHEAD").ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(1)
    })
}

fn estimate_chunk_objective(work_per_seg: &[usize], nthreads: usize, chunk_mult: usize) -> f64 {
    let num_segments = work_per_seg.len();
    if num_segments == 0 || nthreads == 0 || chunk_mult == 0 { return f64::INFINITY; }
    let nchunks = std::cmp::min(num_segments, nthreads * chunk_mult);
    if nchunks == 0 { return f64::INFINITY; }
    let total_work: usize = work_per_seg.iter().sum();
    let target_work = std::cmp::max(1, total_work / nchunks);

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

    let mut max_chunk_work = 0usize;
    for k in 0..(chunk_bounds.len() - 1) {
        let lo = chunk_bounds[k];
        let hi = chunk_bounds[k + 1];
        let mut cw = 0usize;
        for &w in &work_per_seg[lo..hi] { cw += w; }
        if cw > max_chunk_work { max_chunk_work = cw; }
    }

    let avg_work = (total_work as f64) / (num_segments as f64);
    let overhead = (chunk_bounds.len() as f64) * avg_work * 0.03;
    max_chunk_work as f64 + overhead
}

fn choose_runtime_tuning(x: u64) -> RuntimeTuning {
    // Heuristic runtime controller for V10. Environment overrides still take priority.
    if x >= 1_000_000_000_000_000_000 {
        RuntimeTuning { ac_seg: 170_000, ac_par_min: 32, b_chunks: 6, d_chunks: 24, d_adapt_chunks: false }
    } else if x >= 100_000_000_000_000_000 {
        RuntimeTuning { ac_seg: 180_000, ac_par_min: 32, b_chunks: 6, d_chunks: 24, d_adapt_chunks: false }
    } else if x >= 1_000_000_000_000_000 {
        RuntimeTuning { ac_seg: 200_000, ac_par_min: 32, b_chunks: 4, d_chunks: 20, d_adapt_chunks: false }
    } else if x >= 1_000_000_000_000 {
        RuntimeTuning { ac_seg: 200_000, ac_par_min: 64, b_chunks: 2, d_chunks: 12, d_adapt_chunks: false }
    } else {
        RuntimeTuning { ac_seg: 200_000, ac_par_min: 64, b_chunks: 2, d_chunks: 24, d_adapt_chunks: false }
    }
}

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
    #[allow(dead_code)]
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

// ── Large Page support (2MB pages via mimalloc + SeLockMemoryPrivilege) ──────
#[cfg(target_os = "windows")]
mod large_page_alloc {
    use std::ptr;
    use std::sync::Once;

    type HANDLE = *mut std::ffi::c_void;
    type BOOL = i32;
    type DWORD = u32;

    #[repr(C)]
    struct LUID { low_part: u32, high_part: i32 }

    #[repr(C)]
    struct LUID_AND_ATTRIBUTES { luid: LUID, attributes: u32 }

    #[repr(C)]
    struct TOKEN_PRIVILEGES {
        privilege_count: u32,
        privileges: [LUID_AND_ATTRIBUTES; 1],
    }

    #[allow(non_snake_case)]
    extern "system" {
        fn GetCurrentProcess() -> HANDLE;
        fn OpenProcessToken(ProcessHandle: HANDLE, DesiredAccess: DWORD, TokenHandle: *mut HANDLE) -> BOOL;
        fn LookupPrivilegeValueW(lpSystemName: *const u16, lpName: *const u16, lpLuid: *mut LUID) -> BOOL;
        fn AdjustTokenPrivileges(TokenHandle: HANDLE, DisableAll: BOOL, NewState: *const TOKEN_PRIVILEGES, BufferLength: DWORD, PreviousState: *mut TOKEN_PRIVILEGES, ReturnLength: *mut DWORD) -> BOOL;
        fn CloseHandle(hObject: HANDLE) -> BOOL;
    }

    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_QUERY: u32 = 0x0008;
    const SE_PRIVILEGE_ENABLED: u32 = 0x00000002;

    static INIT: Once = Once::new();

    /// Enable SeLockMemoryPrivilege and configure mimalloc for 2MB large pages.
    pub fn enable_large_pages() {
        INIT.call_once(|| {
            // Enable SeLockMemoryPrivilege in process token
            unsafe {
                let process = GetCurrentProcess();
                let mut token: HANDLE = ptr::null_mut();
                if OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) != 0 {
                    let priv_name: Vec<u16> = "SeLockMemoryPrivilege\0".encode_utf16().collect();
                    let mut luid = LUID { low_part: 0, high_part: 0 };
                    if LookupPrivilegeValueW(ptr::null(), priv_name.as_ptr(), &mut luid) != 0 {
                        let tp = TOKEN_PRIVILEGES {
                            privilege_count: 1,
                            privileges: [LUID_AND_ATTRIBUTES { luid, attributes: SE_PRIVILEGE_ENABLED }],
                        };
                        AdjustTokenPrivileges(token, 0, &tp, 0, ptr::null_mut(), ptr::null_mut());
                    }
                    CloseHandle(token);
                }
            }
            // Tell mimalloc to use large OS pages for all future allocations
            extern "C" { fn mi_option_set(option: i32, value: i64); }
            const MI_OPTION_ALLOW_LARGE_OS_PAGES: i32 = 6;
            unsafe { mi_option_set(MI_OPTION_ALLOW_LARGE_OS_PAGES, 1); }
        });
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

fn generate_tables(limit: usize, y: usize) -> (Vec<i8>, Vec<u16>, Vec<bool>) {
    let mut mu = vec![1i8; limit + 1];
    let mut lpf = vec![0u16; limit + 1];
    let mut y_smooth = vec![true; limit + 1];
    mu[0] = 0;
    y_smooth[0] = false;

    let sqrt_limit = isqrt(limit as u64) as usize;

    let small_sieve = Sieve::new(sqrt_limit);
    let small_primes: Vec<usize> = small_sieve.primes_from(2)
        .take_while(|&p| p <= sqrt_limit).collect();

    // Phase 1: Parallel segmented sieve — small primes mark mu/lpf per segment
    // Each segment's mu(S)+lpf(2S) ≈ 1.5MB fits in L2
    let seg_size: usize = 512 * 1024;
    let n = limit + 1;
    let mu_raw = mu.as_mut_ptr() as usize;
    let lpf_raw = lpf.as_mut_ptr() as usize;
    let num_segs = (n + seg_size - 1) / seg_size;

    (0..num_segs).into_par_iter().for_each(move |seg| {
        let lo = seg * seg_size;
        let hi = std::cmp::min(lo + seg_size, n);
        let mu_p = mu_raw as *mut i8;
        let lpf_p = lpf_raw as *mut u16;

        unsafe {
            for &p in &small_primes {
                let p_u16 = p as u16;
                let first = std::cmp::max(2 * p, ((lo + p - 1) / p) * p);
                let mut m = first;
                while m < hi {
                    if *lpf_p.add(m) == 0 { *lpf_p.add(m) = p_u16; }
                    *mu_p.add(m) = -*mu_p.add(m);
                    m += p;
                }
                let p2 = p * p;
                if p2 < hi {
                    let first2 = std::cmp::max(p2, ((lo + p2 - 1) / p2) * p2);
                    let mut m2 = first2;
                    while m2 < hi {
                        *mu_p.add(m2) = 0;
                        m2 += p2;
                    }
                }
            }
        }
    });

    // Phase 2: identify primes, then parallel medium/large prime marking
    // No two medium primes share a multiple (p1*p2 > z), so parallel is race-free
    let all_primes: Vec<usize> = (2..=limit).filter(|&p| lpf[p] == 0).collect();
    for &p in &all_primes {
        let p_u16 = std::cmp::min(p, u16::MAX as usize) as u16;
        lpf[p] = p_u16;
        mu[p] = -1;
    }

    // Parallel marking: no two medium primes share a multiple (p1*p2 > z)
    let mu_raw2 = mu.as_mut_ptr() as usize;
    let ys_raw = y_smooth.as_mut_ptr() as usize;
    let limit_copy = limit;
    let y_copy = y;
    all_primes.par_iter()
        .filter(|&&p| p > sqrt_limit)
        .for_each(|&p| {
            let mu_p = mu_raw2 as *mut i8;
            unsafe {
                let mut m = 2 * p;
                while m <= limit_copy {
                    *mu_p.add(m) = -*mu_p.add(m);
                    m += p;
                }
            }
            if p > y_copy {
                let ys_p = ys_raw as *mut u8;
                unsafe {
                    let mut m = p;
                    while m <= limit_copy {
                        *ys_p.add(m) = 0;
                        m += p;
                    }
                }
            }
        });

    (mu, lpf, y_smooth)
}

// Fast pi table generation from a raw bit sieve (avoids primal::Sieve per-call overhead).
// sieve_bits: odd-only sieve where bit i represents number 2*i+3.
// Two-level compact pi table: coarse (u32 per STRIDE) + fine (u8 per entry).
// Size: ~51MB instead of ~200MB for pi_table_limit ≈ 50M entries.
// Reduces D's L3 footprint, leaving more cache for AC's BigPiTable.
const PI_STRIDE: usize = 256;

struct CompactPi {
    coarse: Vec<u32>,  // coarse[i] = pi(i * PI_STRIDE)
    fine: Vec<u8>,     // fine[n] = pi(n) - coarse[n / PI_STRIDE]
    limit: usize,
}

impl CompactPi {
    #[inline(always)]
    fn get(&self, n: usize) -> u32 {
        debug_assert!(n <= self.limit);
        unsafe {
            *self.coarse.get_unchecked(n / PI_STRIDE) + *self.fine.get_unchecked(n) as u32
        }
    }

    fn len(&self) -> usize { self.limit + 1 }
}

fn generate_compact_pi(limit: usize, sieve_bits: &[u64]) -> CompactPi {
    let coarse_len = limit / PI_STRIDE + 2;
    let mut coarse = vec![0u32; coarse_len];
    let mut fine = vec![0u8; limit + 1];
    if limit < 2 {
        return CompactPi { coarse, fine, limit };
    }

    // Single-pass: compute pi running sum while filling coarse+fine
    let mut count = 0u32;
    for n in 0..=limit {
        // Check if n is prime and update count
        if n == 2 {
            count += 1;
        } else if n >= 3 && n % 2 == 1 {
            let idx = (n - 3) / 2;
            count += ((sieve_bits[idx / 64] >> (idx % 64)) & 1) as u32;
        }
        // Set coarse at stride boundaries (before computing fine)
        let ci = n / PI_STRIDE;
        if n % PI_STRIDE == 0 && ci < coarse_len {
            coarse[ci] = count;
        }
        // Store fine = pi(n) - coarse[n/STRIDE]
        fine[n] = (count - coarse[ci]) as u8;
    }
    CompactPi { coarse, fine, limit }
}

// Fast odd-only sieve of Eratosthenes returning raw bit array.
// Bit i represents number 2*i+3. 1 = prime.
fn fast_bit_sieve(limit: usize) -> Vec<u64> {
    if limit < 3 { return vec![]; }
    let nbits = (limit - 3) / 2 + 1;
    let nwords = (nbits + 63) / 64;
    let mut bits = vec![u64::MAX; nwords];
    // Clear trailing bits
    let trailing = nbits % 64;
    if trailing != 0 {
        bits[nwords - 1] &= (1u64 << trailing) - 1;
    }
    // Sieve
    let sqrt_limit = isqrt(limit as u64) as usize;
    let mut p = 3usize;
    while p <= sqrt_limit {
        let pidx = (p - 3) / 2;
        if bits[pidx / 64] & (1u64 << (pidx % 64)) != 0 {
            let mut m = p * p;
            while m <= limit {
                let midx = (m - 3) / 2;
                bits[midx / 64] &= !(1u64 << (midx % 64));
                m += 2 * p;
            }
        }
        p += 2;
    }
    bits
}

// Collect primes up to max_prime from a fast bit sieve.
fn collect_primes_from_bits(sieve_bits: &[u64], max_prime: usize) -> Vec<u32> {
    let mut primes = vec![0u32, 2]; // 0-indexed sentinel + 2
    for w in 0..sieve_bits.len() {
        let mut word = sieve_bits[w];
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            let p = 2 * (w * 64 + bit) + 3;
            if p > max_prime { return primes; }
            primes.push(p as u32);
            word &= word - 1; // clear lowest set bit
        }
    }
    primes
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
        (43.6, 18.5, 1.5),   // x ~ Max i64 (retuned in V9)
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

    // Allow env var overrides for alpha parameter sweeps
    let alpha_y = std::env::var("ALPHA_Y").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(alpha_y);
    let alpha_z = std::env::var("ALPHA_Z").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(alpha_z);

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

            if seg == 0 {
                local[0] |= (1u64 << 1) | (1u64 << 2) | (1u64 << 3);
            }

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

        let mut all_bits = Vec::with_capacity(nwords);
        for seg in &seg_results {
            all_bits.extend_from_slice(seg);
        }
        all_bits.truncate(nwords);

        let mut prefix = vec![0u32; nwords];
        let mut running = 1u32; // +1 accounts for prime 2 (not in odd sieve), saves 1 ADD in pi_fast
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
        prefix + (*self.bits.get_unchecked(word) & mask).count_ones() as u64
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
    #[allow(dead_code)]
    fn prefetch(&self, n: usize) {
        if n >= 3 {
            let w = (n - 1) / 2 / 64;
            unsafe {
                _mm_prefetch(self.bits.as_ptr().add(w) as *const i8, _MM_HINT_T0);
                _mm_prefetch(self.prefix.as_ptr().add(w) as *const i8, _MM_HINT_T0);
            }
        }
    }

    /// Find the smallest prime > n using the sieve bits.
    #[inline]
    #[allow(dead_code)]
    fn next_prime_after(&self, n: usize) -> usize {
        if n < 2 { return 2; }
        if n < 3 { return 3; }
        // Start searching from the next odd number
        let start = if n % 2 == 0 { n + 1 } else { n + 2 };
        let odd_idx = (start - 1) / 2;
        let word = odd_idx >> 6;
        let bit = odd_idx & 63;
        // Check remaining bits in current word
        unsafe {
            let mask = *self.bits.get_unchecked(word) >> bit;
            if mask != 0 {
                return 2 * (word * 64 + bit + mask.trailing_zeros() as usize) + 1;
            }
            // Search subsequent words
            for w in (word + 1)..self.bits.len() {
                let bits = *self.bits.get_unchecked(w);
                if bits != 0 {
                    return 2 * (w * 64 + bits.trailing_zeros() as usize) + 1;
                }
            }
        }
        unreachable!("no prime found after {}", n)
    }
}

// ── Sigma: 7 correction formulas ─────────────────────────────────────────────

fn compute_sigma(x: u64, y: usize, x_star: usize,
                 _primes: &[u32], pi: &CompactPi, big_pi: &BigPiTable) -> i64 {
    let pi_limit = pi.len() - 1;
    let a = pi.get(y) as i64;
    let x13 = icbrt(x) as usize;
    let b = pi.get(std::cmp::min(x13, pi_limit)) as i64;
    let sqrt_xy = isqrt(x / y as u64) as usize;
    let c = pi.get(std::cmp::min(sqrt_xy, pi_limit)) as i64;
    let d = pi.get(std::cmp::min(x_star, pi_limit)) as i64;
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
            sigma4 += pi.get(std::cmp::min(xpy, pi_table_limit)) as i64;
        } else {
            // Σ₅: √(x/y) < p ≤ x^{1/3}
            let xpp = (x / (p64 * p64)) as usize;
            sigma5 += pi.get(std::cmp::min(xpp, pi_table_limit)) as i64;
        }

        // Σ₆: x* < p ≤ x^{1/3}
        let sqrt_xp = isqrt(x / p64) as usize;
        let pi_sqrt_xp = pi.get(std::cmp::min(sqrt_xp, pi_table_limit)) as i64;
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

    // Pre-allocate: avoids repeated doubling/copying for ~140M entries at max i64
    let estimated_primes = (big_pi.pi(sqrt_x) - big_pi.pi(y)) as usize;
    let mut xp_asc: Vec<u64> = Vec::with_capacity(estimated_primes);
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
    let b_chunks_mult: usize = std::env::var("B_CHUNKS").ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| get_runtime_tuning().map(|t| t.b_chunks))
        .unwrap_or(4);
    let nchunks = (nthreads * b_chunks_mult).max(1);
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
                mu_sign: i64, primes: &[u32], pi: &CompactPi) -> i64 {
    let mut sum = 0i64;
    let y_limit = pi.len() - 1;
    for j in (i + 1)..=pi_y {
        let next = m as u128 * primes[j] as u128;
        if next > max_m as u128 { return sum; }
        let next = next as u64;

        if next as usize > min_m {
            let xpm = (xp / next) as usize;
            let phi_xpm = pi.get(std::cmp::min(xpm, y_limit)) as i64 - b as i64 + 2;
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

fn compute_c1(x: u64, y: usize, z: usize, k: usize,
              primes: &[u32], pi: &CompactPi) -> i64 {
    let pi_limit = pi.len() - 1;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_y = pi.get(y) as usize;
    let pi_sqrtz = pi.get(std::cmp::min(sqrt_z, pi_limit)) as usize;
    let pi_root3_xz = pi.get(std::cmp::min(icbrt(x / z as u64) as usize, pi_limit)) as usize;
    let min_c1_b = std::cmp::max(k, pi_root3_xz) + 1;

    let c1_range: Vec<usize> = (min_c1_b..=pi_sqrtz)
        .filter(|&b| b < primes.len())
        .collect();
    c1_range.par_iter().map(|&b| {
        let prime = primes[b] as u64;
        let xp = x / prime;
        let max_m = std::cmp::min((xp / prime) as usize, z);
        let min_m_val = std::cmp::max((xp / (prime * prime)) as usize, z / prime as usize);
        let min_m_val = std::cmp::min(min_m_val, max_m);
        -c1_recursive(xp, b, b, pi_y, 1, min_m_val, max_m, -1, primes, pi)
    }).sum()
}

// ── AC: combined A + C formulas ──────────────────────────────────────────────
// V9: b-first iteration with BigPiTable (proven V7 structure) plus
// full-range SegmentedPiTable experiment. Uses mod-240 wheel encoding
// (single struct access per pi lookup vs BigPiTable's two-array access).
// C1: recursive Möbius (few iterations, computed separately)
// C2: easy leaves (π(√z) < b ≤ π(x*))
// A:  simplest easy leaves (π(x*) < b ≤ π(x^{1/3}))

fn compute_ac(x: u64, y: usize, z: usize, k: usize, x_star: usize,
              primes: &[u32], pi: &CompactPi, big_pi: &BigPiTable,
              recip: &[u64]) -> i64 {
    let pi_limit = pi.len() - 1;
    let x13 = icbrt(x) as usize;
    let sqrt_x = isqrt(x) as usize;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_sqrtz = pi.get(std::cmp::min(sqrt_z, pi_limit)) as usize;
    let pi_x_star = pi.get(std::cmp::min(x_star, pi_limit)) as usize;
    let pi_x13 = pi.get(std::cmp::min(x13, pi_limit)) as usize;
    let pi_root3_xy = pi.get(std::cmp::min(icbrt(x / y as u64) as usize, pi_limit)) as usize;

    let min_c2_b = std::cmp::max(k, std::cmp::max(pi_root3_xy, pi_sqrtz)) + 1;

    // Build per-b info for C2 and A
    struct BLookup {
        b: usize,
        xp: u64,
        l_cur: usize,
        l_max: usize,
        y_boundary_l: usize,
        is_c2: bool,
    }
    #[derive(Copy, Clone)]
    struct HotLookup {
        b_term: i32,        // for C2: (-b + 2), ignored for A
        xp: u64,
        l_cur: u32,
        l_max: u32,
        y_boundary_l: u32,  // u32::MAX for C2
        is_c2: u8,          // 1 for C2, 0 for A
    }

    let mut b_lookups: Vec<BLookup> = Vec::new();

    // C2 b values
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
        let l_max = pi.get(std::cmp::min(max_m, pi_limit)) as usize;
        let l_min = pi.get(std::cmp::min(min_m_val, pi_limit)) as usize + 1;
        if l_min > l_max { continue; }
        b_lookups.push(BLookup { b, xp, l_cur: l_min, l_max, y_boundary_l: usize::MAX, is_c2: true });
    }

    // A b values
    for b in (pi_x_star + 1)..=pi_x13 {
        if b >= primes.len() { continue; }
        let prime = primes[b] as u64;
        let xp = x / prime;
        let sqrt_xp = isqrt(xp) as usize;
        let max_2nd = std::cmp::min(sqrt_xp, y);
        let min_2nd = std::cmp::max(prime as usize, 1);
        if max_2nd <= min_2nd { continue; }
        let max_i = pi.get(std::cmp::min(max_2nd, pi_limit)) as usize;
        let min_i = pi.get(std::cmp::min(min_2nd, pi_limit)) as usize + 1;
        if min_i > max_i { continue; }
        let xp_over_y = (xp / y as u64) as usize;
        let y_boundary_l = pi.get(std::cmp::min(std::cmp::min(xp_over_y, max_2nd), pi_limit)) as usize;
        b_lookups.push(BLookup { b, xp, l_cur: min_i, l_max: max_i, y_boundary_l, is_c2: false });
    }

    let c2_a_sum: i64 = if b_lookups.is_empty() { 0 } else {
        let primes_len = primes.len();

        // Segmented AC: process BigPiTable in L2-cache-sized segments.
        let seg_pairs: usize = std::env::var("AC_SEG").ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| get_runtime_tuning().map(|t| t.ac_seg))
            .unwrap_or(170_000);
        let total_pairs = big_pi.bits.len();
        let num_segs = (total_pairs + seg_pairs - 1) / seg_pairs;
        let numbers_per_seg = seg_pairs * 128;

        // Partition b_lookups into narrow (xpq range fits in ~1 segment) and wide.
        let mut wide_ranges: Vec<(u32, u32, u32)> = Vec::new(); // (b_idx, seg_lo, seg_hi)
        let mut narrow_seg_assign: Vec<(u32, u32)> = Vec::new();
        for (i, info) in b_lookups.iter().enumerate() {
            let xpq_min = info.xp / primes[std::cmp::min(info.l_max, primes_len - 1)] as u64;
            let xpq_max = info.xp / primes[info.l_cur] as u64;
            if (xpq_max - xpq_min) < numbers_per_seg as u64 {
                let seg = std::cmp::min(xpq_min as usize / numbers_per_seg, num_segs - 1);
                narrow_seg_assign.push((i as u32, seg as u32));
            } else {
                let seg_lo = std::cmp::min(xpq_min as usize / numbers_per_seg, num_segs - 1) as u32;
                let seg_hi = std::cmp::min(xpq_max as usize / numbers_per_seg, num_segs - 1) as u32;
                wide_ranges.push((i as u32, seg_lo, seg_hi));
            }
        }
        // Sort narrow by (segment, xpq midpoint) for BigPiTable cache locality
        narrow_seg_assign.sort_unstable_by_key(|&(i, seg)| {
            let info = &b_lookups[i as usize];
            let mid_l = (info.l_cur + info.l_max) / 2;
            let xpq_mid = info.xp / primes[std::cmp::min(mid_l, primes_len - 1)] as u64;
            (seg, xpq_mid)
        });
        // Event-sweep for wide b-values:
        // iterate segments high->low, activate ranges at seg_hi and retire at seg_lo.
        let mut wide_start_by_hi: Vec<Vec<u32>> = (0..num_segs).map(|_| Vec::new()).collect();
        let mut wide_retire_by_lo: Vec<Vec<u32>> = (0..num_segs).map(|_| Vec::new()).collect();
        for &(bi, seg_lo, seg_hi) in &wide_ranges {
            wide_start_by_hi[seg_hi as usize].push(bi);
            wide_retire_by_lo[seg_lo as usize].push(bi);
        }
        let mut active_wide: Vec<u32> = Vec::new();
        let mut active_pos: Vec<usize> = vec![usize::MAX; b_lookups.len()];

        let hot_lookups: Vec<HotLookup> = b_lookups.iter().map(|info| HotLookup {
            b_term: 2 - info.b as i32,
            xp: info.xp,
            l_cur: info.l_cur as u32,
            l_max: info.l_max as u32,
            y_boundary_l: if info.is_c2 { u32::MAX } else { info.y_boundary_l as u32 },
            is_c2: if info.is_c2 { 1 } else { 0 },
        }).collect();

        let t_ac_loops = std::time::Instant::now();
        let ac_par_min: usize = std::env::var("AC_PAR_MIN").ok()
            .and_then(|s| s.parse().ok())
            .or_else(|| get_runtime_tuning().map(|t| t.ac_par_min))
            .unwrap_or(32);

        let mut combined_sum: i64 = 0;
        for seg in (0..num_segs).rev() {
            for &bi in &wide_start_by_hi[seg] {
                let biu = bi as usize;
                if unsafe { *active_pos.get_unchecked(biu) } == usize::MAX {
                    let pos = active_wide.len();
                    active_wide.push(bi);
                    unsafe { *active_pos.get_unchecked_mut(biu) = pos; }
                }
            }

            let n_lo = if seg == 0 { 0usize } else { seg * numbers_per_seg };
            let n_hi = std::cmp::min((seg + 1) * numbers_per_seg - 1, sqrt_x);

            let narrow_start = narrow_seg_assign.partition_point(|&(_, s)| (s as usize) < seg);
            let narrow_end = narrow_seg_assign.partition_point(|&(_, s)| (s as usize) <= seg);
            let n_narrow = narrow_end - narrow_start;
            let n_wide = active_wide.len();
            let n_total = n_wide + n_narrow;
            if n_total == 0 { continue; }

            let process_idx = |idx: usize| -> i64 {
                let bi = if idx < n_wide {
                    unsafe { *active_wide.get_unchecked(idx) }
                } else {
                    unsafe { narrow_seg_assign.get_unchecked(narrow_start + idx - n_wide).0 }
                };
                let info = unsafe { *hot_lookups.get_unchecked(bi as usize) };
                let is_narrow = idx >= n_wide;

                let (eff_lo, eff_hi) = if is_narrow {
                    (info.l_cur as usize, std::cmp::min(info.l_max as usize, primes_len - 1))
                } else {
                    let l_lo = if seg == num_segs - 1 {
                        info.l_cur as usize
                    } else {
                        let thresh = std::cmp::min(
                            (info.xp / (n_hi as u64 + 1)) as usize, pi_limit);
                        let l_candidate = pi.get(thresh) as usize + 1;
                        std::cmp::max(l_candidate, info.l_cur as usize)
                    };

                    let l_hi = if n_lo <= 1 {
                        info.l_max as usize
                    } else {
                        let thresh_raw = info.xp / n_lo as u64;
                        if thresh_raw == 0 { return 0; }
                        let thresh = std::cmp::min(thresh_raw as usize, pi_limit);
                        let l_candidate = pi.get(thresh) as usize;
                        std::cmp::min(l_candidate, info.l_max as usize)
                    };

                    (std::cmp::max(l_lo, info.l_cur as usize),
                     std::cmp::min(l_hi, std::cmp::min(info.l_max as usize, primes_len - 1)))
                };
                if eff_lo > eff_hi || eff_lo >= primes_len { return 0; }

                let mut local = 0i64;
                let mut l = eff_lo;
                while l + 3 <= eff_hi {
                    unsafe {
                    let xpq0 = fast_div(info.xp, *primes.get_unchecked(l) as u64, *recip.get_unchecked(l)) as usize;
                    let xpq1 = fast_div(info.xp, *primes.get_unchecked(l+1) as u64, *recip.get_unchecked(l+1)) as usize;
                    let xpq2 = fast_div(info.xp, *primes.get_unchecked(l+2) as u64, *recip.get_unchecked(l+2)) as usize;
                    let xpq3 = fast_div(info.xp, *primes.get_unchecked(l+3) as u64, *recip.get_unchecked(l+3)) as usize;

                        if info.is_c2 != 0 {
                            local += (big_pi.pi_fast(xpq0) as i64 + info.b_term as i64)
                                   + (big_pi.pi_fast(xpq1) as i64 + info.b_term as i64)
                                   + (big_pi.pi_fast(xpq2) as i64 + info.b_term as i64)
                                   + (big_pi.pi_fast(xpq3) as i64 + info.b_term as i64);
                        } else {
                            let p0 = big_pi.pi_fast(xpq0) as i64;
                            let p1 = big_pi.pi_fast(xpq1) as i64;
                            let p2 = big_pi.pi_fast(xpq2) as i64;
                            let p3 = big_pi.pi_fast(xpq3) as i64;
                            let yb = info.y_boundary_l as usize;
                            if l + 3 <= yb {
                                local += p0 + p1 + p2 + p3;
                            } else if l > yb {
                                local += 2 * (p0 + p1 + p2 + p3);
                            } else {
                                for (ll, pv) in [(l, p0), (l + 1, p1), (l + 2, p2), (l + 3, p3)] {
                                    local += if ll <= yb { pv } else { 2 * pv };
                                }
                            }
                        }
                    }
                    l += 4;
                }

                while l <= eff_hi {
                    let xpq = unsafe { fast_div(info.xp, *primes.get_unchecked(l) as u64, *recip.get_unchecked(l)) } as usize;
                    let pi_val = unsafe { big_pi.pi_fast(xpq) } as i64;
                    if info.is_c2 != 0 {
                        local += pi_val + info.b_term as i64;
                    } else if l <= info.y_boundary_l as usize {
                        local += pi_val;
                    } else {
                        local += 2 * pi_val;
                    }
                    l += 1;
                }

                local
            };
            let seg_sum: i64 = if n_total < ac_par_min {
                let mut s = 0i64;
                for idx in 0..n_total {
                    s += process_idx(idx);
                }
                s
            } else {
                (0..n_total).into_par_iter().map(process_idx).sum()
            };

            combined_sum += seg_sum;

                // Next iteration is seg-1, so entries with seg_lo == seg expire now.
                for &bi in &wide_retire_by_lo[seg] {
                    let biu = bi as usize;
                    let pos = unsafe { *active_pos.get_unchecked(biu) };
                    if pos != usize::MAX {
                        let last_idx = active_wide.len() - 1;
                        let moved = unsafe { *active_wide.get_unchecked(last_idx) };
                        active_wide.swap_remove(pos);
                        if pos < active_wide.len() {
                            unsafe { *active_pos.get_unchecked_mut(moved as usize) = pos; }
                        }
                        unsafe { *active_pos.get_unchecked_mut(biu) = usize::MAX; }
                    }
                }
        }
        let ac_elapsed = t_ac_loops.elapsed().as_secs_f64();

        if std::env::var("SHOW_TIMING").is_ok() {
            eprintln!("    AC loops: {:.3}s ({} wide + {} narrow b-values, {} segs)",
                ac_elapsed, wide_ranges.len(), narrow_seg_assign.len(), num_segs);
        }

        combined_sum
    };

    c2_a_sum
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

// Wheel cross-off tables for byte-level sieve stepping
const WHEEL_GAPS: [usize; 8] = [6, 4, 2, 4, 2, 4, 6, 2];
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
        let nwords = (self.len + 63) / 64;

        // Pick shorter direction: forward from start or backward from end
        if full < nwords / 2 {
            // Forward scan
            let mut cnt = 0u64;
            for i in 0..full {
                cnt += unsafe { *self.bits.get_unchecked(i) }.count_ones() as u64;
            }
            let mask = (2u64 << bit) - 1;
            cnt += (unsafe { *self.bits.get_unchecked(full) } & mask).count_ones() as u64;
            cnt as i64
        } else {
            // Backward scan: total - count_of_bits_after_pos
            let mut suffix = 0u64;
            let after_mask = if bit < 63 { u64::MAX << (bit + 1) } else { 0 };
            suffix += (unsafe { *self.bits.get_unchecked(full) } & after_mask).count_ones() as u64;
            for i in (full + 1)..nwords {
                suffix += unsafe { *self.bits.get_unchecked(i) }.count_ones() as u64;
            }
            self.total - suffix as i64
        }
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
    let r_idx = MOD30_TO_IDX[p % 30] as usize;
    let k = p / 30;
    let sieve_bytes = (sieve.len + 7) / 8;
    let bytes = sieve.bits.as_mut_ptr() as *mut u8;

    // Per-step byte advances and bit masks for the 8 wheel positions
    let adv = [
        k * WHEEL_GAPS[0] + WHEEL_CORR[r_idx][0],
        k * WHEEL_GAPS[1] + WHEEL_CORR[r_idx][1],
        k * WHEEL_GAPS[2] + WHEEL_CORR[r_idx][2],
        k * WHEEL_GAPS[3] + WHEEL_CORR[r_idx][3],
        k * WHEEL_GAPS[4] + WHEEL_CORR[r_idx][4],
        k * WHEEL_GAPS[5] + WHEEL_CORR[r_idx][5],
        k * WHEEL_GAPS[6] + WHEEL_CORR[r_idx][6],
        k * WHEEL_GAPS[7] + WHEEL_CORR[r_idx][7],
    ];
    let msk = [
        1u8 << WHEEL_BITS[r_idx][0],
        1u8 << WHEEL_BITS[r_idx][1],
        1u8 << WHEEL_BITS[r_idx][2],
        1u8 << WHEEL_BITS[r_idx][3],
        1u8 << WHEEL_BITS[r_idx][4],
        1u8 << WHEEL_BITS[r_idx][5],
        1u8 << WHEEL_BITS[r_idx][6],
        1u8 << WHEEL_BITS[r_idx][7],
    ];

    // Find first coprime-to-30 q with p*q > low
    let min_q = std::cmp::max((low + p) / p, 1);
    let base_q = (min_q / 30) * 30;
    let (mut m, start_s) = 'find: {
        for cycle in 0..2usize {
            let cbase = base_q + cycle * 30;
            for s in 0..8usize {
                let q = cbase + WHEEL30_RESIDUES[s];
                if q >= min_q {
                    let composite = p * q;
                    if composite >= high { return; }
                    break 'find ((composite - low) / 30, s);
                }
            }
        }
        return;
    };

    let mut delta = 0i64;
    let mut s = start_s;

    // Peel: align to cycle start (step 0)
    while s != 0 && m < sieve_bytes {
        unsafe {
            let b = *bytes.add(m);
            delta += ((b & msk[s]) != 0) as i64;
            *bytes.add(m) = b & !msk[s];
        }
        m += adv[s];
        s = (s + 1) & 7;
    }

    // Full unrolled wheel cycles (8 crossings each, single pass through sieve)
    // Last access in a cycle starting at m is at m + adv[0]+...+adv[6]
    let partial = adv[0] + adv[1] + adv[2] + adv[3] + adv[4] + adv[5] + adv[6];
    if sieve_bytes > partial {
        let limit = sieve_bytes - partial;
        while m < limit {
            unsafe {
                let b0 = *bytes.add(m); delta += ((b0 & msk[0]) != 0) as i64;
                *bytes.add(m) = b0 & !msk[0]; m += adv[0];
                let b1 = *bytes.add(m); delta += ((b1 & msk[1]) != 0) as i64;
                *bytes.add(m) = b1 & !msk[1]; m += adv[1];
                let b2 = *bytes.add(m); delta += ((b2 & msk[2]) != 0) as i64;
                *bytes.add(m) = b2 & !msk[2]; m += adv[2];
                let b3 = *bytes.add(m); delta += ((b3 & msk[3]) != 0) as i64;
                *bytes.add(m) = b3 & !msk[3]; m += adv[3];
                let b4 = *bytes.add(m); delta += ((b4 & msk[4]) != 0) as i64;
                *bytes.add(m) = b4 & !msk[4]; m += adv[4];
                let b5 = *bytes.add(m); delta += ((b5 & msk[5]) != 0) as i64;
                *bytes.add(m) = b5 & !msk[5]; m += adv[5];
                let b6 = *bytes.add(m); delta += ((b6 & msk[6]) != 0) as i64;
                *bytes.add(m) = b6 & !msk[6]; m += adv[6];
                let b7 = *bytes.add(m); delta += ((b7 & msk[7]) != 0) as i64;
                *bytes.add(m) = b7 & !msk[7]; m += adv[7];
            }
        }
    }

    // Tail: remaining crossings with bounds checks
    s = 0;
    while m < sieve_bytes {
        unsafe {
            let b = *bytes.add(m);
            delta += ((b & msk[s]) != 0) as i64;
            *bytes.add(m) = b & !msk[s];
        }
        m += adv[s];
        s = (s + 1) & 7;
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

#[derive(Clone, Default)]
struct DVmStats {
    queries: u64,
    same_bucket_queries: u64,
    cross_bucket_queries: u64,
    empty_queries: u64,
    bucket_delta_sum: u64,
    bucket_delta_hist: [u64; 6],
    same_bucket_search_items: u64,
    same_bucket_result_items: u64,
    cross_bucket_start_items: u64,
    cross_bucket_end_items: u64,
    cross_bucket_result_items: u64,
}

impl DVmStats {
    #[inline(always)]
    fn bucket_delta_bin(delta: usize) -> usize {
        match delta {
            0 => 0,
            1 => 1,
            2 => 2,
            3 | 4 => 3,
            5..=8 => 4,
            _ => 5,
        }
    }

    #[inline(always)]
    fn record_same_bucket(&mut self, delta: usize, search_items: usize, result_items: usize) {
        self.queries += 1;
        self.same_bucket_queries += 1;
        self.bucket_delta_sum += delta as u64;
        self.bucket_delta_hist[Self::bucket_delta_bin(delta)] += 1;
        self.same_bucket_search_items += search_items as u64;
        self.same_bucket_result_items += result_items as u64;
        if result_items == 0 {
            self.empty_queries += 1;
        }
    }

    #[inline(always)]
    fn record_cross_bucket(
        &mut self,
        delta: usize,
        start_items: usize,
        end_items: usize,
        result_items: usize,
    ) {
        self.queries += 1;
        self.cross_bucket_queries += 1;
        self.bucket_delta_sum += delta as u64;
        self.bucket_delta_hist[Self::bucket_delta_bin(delta)] += 1;
        self.cross_bucket_start_items += start_items as u64;
        self.cross_bucket_end_items += end_items as u64;
        self.cross_bucket_result_items += result_items as u64;
        if result_items == 0 {
            self.empty_queries += 1;
        }
    }

    fn merge(&mut self, other: &Self) {
        self.queries += other.queries;
        self.same_bucket_queries += other.same_bucket_queries;
        self.cross_bucket_queries += other.cross_bucket_queries;
        self.empty_queries += other.empty_queries;
        self.bucket_delta_sum += other.bucket_delta_sum;
        self.same_bucket_search_items += other.same_bucket_search_items;
        self.same_bucket_result_items += other.same_bucket_result_items;
        self.cross_bucket_start_items += other.cross_bucket_start_items;
        self.cross_bucket_end_items += other.cross_bucket_end_items;
        self.cross_bucket_result_items += other.cross_bucket_result_items;
        for i in 0..self.bucket_delta_hist.len() {
            self.bucket_delta_hist[i] += other.bucket_delta_hist[i];
        }
    }

    fn print(&self, num_segments: usize) {
        if self.queries == 0 {
            eprintln!("    D VM stats: no Type 1 sparse-index queries across {} segments", num_segments);
            return;
        }

        let q = self.queries as f64;
        let same_q = self.same_bucket_queries.max(1) as f64;
        let cross_q = self.cross_bucket_queries.max(1) as f64;

        eprintln!(
            "    D VM stats: queries={} same={} ({:.1}%) cross={} ({:.1}%) empty={} ({:.1}%) avg_bucket_delta={:.2}",
            self.queries,
            self.same_bucket_queries,
            100.0 * self.same_bucket_queries as f64 / q,
            self.cross_bucket_queries,
            100.0 * self.cross_bucket_queries as f64 / q,
            self.empty_queries,
            100.0 * self.empty_queries as f64 / q,
            self.bucket_delta_sum as f64 / q,
        );
        eprintln!(
            "      bucket delta hist: [0]={} [1]={} [2]={} [3-4]={} [5-8]={} [9+]={}",
            self.bucket_delta_hist[0],
            self.bucket_delta_hist[1],
            self.bucket_delta_hist[2],
            self.bucket_delta_hist[3],
            self.bucket_delta_hist[4],
            self.bucket_delta_hist[5],
        );
        eprintln!(
            "      same-bucket avg search items={:.1} avg result items={:.1}",
            self.same_bucket_search_items as f64 / same_q,
            self.same_bucket_result_items as f64 / same_q,
        );
        eprintln!(
            "      cross-bucket avg start items={:.1} avg end items={:.1} avg result items={:.1}",
            self.cross_bucket_start_items as f64 / cross_q,
            self.cross_bucket_end_items as f64 / cross_q,
            self.cross_bucket_result_items as f64 / cross_q,
        );
    }
}

fn build_valid_m(z: usize, k: usize, primes: &[u32], pi: &CompactPi,
                 mu: &[i8], lpf: &[u16], y_smooth: &[bool]) -> (Vec<ValidM>, Vec<u32>) {
    let vm_stride = get_vm_stride();
    let pi_limit = pi.len() - 1;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_sqrtz = pi.get(std::cmp::min(sqrt_z, pi_limit)) as usize;
    let c = k;
    let min_lpf_threshold = if c + 1 < primes.len() { primes[c + 1] as u16 } else { u16::MAX };
    let valid_m_list: Vec<ValidM> = if pi_sqrtz > c {
        (1..=z).into_par_iter()
            .filter_map(|m| {
                if m < mu.len() && mu[m] != 0 && lpf[m] > min_lpf_threshold && y_smooth[m] {
                    Some(ValidM {
                        m: m as u32,
                        lpf: lpf[m],
                        mu_val: mu[m],
                        _pad: 0,
                        recip_m: ((1u128 << 64) / m as u128) as u64,
                    })
                } else { None }
            }).collect()
    } else { vec![] };

    let vm_index: Vec<u32> = if !valid_m_list.is_empty() {
        let max_m = valid_m_list.last().unwrap().m as usize;
        let index_len = max_m / vm_stride + 2;
        let mut idx = vec![valid_m_list.len() as u32; index_len];
        let mut vi = 0usize;
        for bucket in 0..index_len {
            let bucket_start = bucket * vm_stride;
            while vi < valid_m_list.len() && (valid_m_list[vi].m as usize) < bucket_start {
                vi += 1;
            }
            idx[bucket] = vi as u32;
        }
        idx
    } else { vec![] };

    (valid_m_list, vm_index)
}

fn compute_d(x: u64, y: usize, z: usize, k: usize, x_star: usize,
             primes: &[u32], pi: &CompactPi,
             valid_m_list: &[ValidM], vm_index: &[u32],
             prime_recip: &[u64]) -> i64 {
    if z == 0 { return 0; }
    let vm_stride = get_vm_stride();
    let vm_lookahead = get_vm_lookahead();

    let xz = (x / z as u64) as usize;
    let sqrt_z = isqrt(z as u64) as usize;
    let pi_limit = pi.len() - 1;
    let pi_sqrtz = pi.get(std::cmp::min(sqrt_z, pi_limit)) as usize;
    let pi_x_star = pi.get(std::cmp::min(x_star, pi_limit)) as usize;
    let nprimes = primes.len();
    let c = k;

    if c >= pi_x_star { return 0; }

    let template = PreSieveTemplate::new(primes, std::cmp::min(c, nprimes - 1));

    let target_segs = rayon::current_num_threads() * 32;
    let seg_cap = std::env::var("D_SEG_CAP").ok()
        .and_then(|s| s.parse::<u32>().ok()).unwrap_or(20);
    let seg_min_cap = std::env::var("D_SEG_MIN_CAP").ok()
        .and_then(|s| s.parse::<u32>().ok()).unwrap_or(14);
    // Round segment_size to multiple of 30 for wheel-30 alignment
    let segment_size_raw = std::cmp::max(
        std::cmp::min(xz / std::cmp::max(target_segs, 1), 1usize << seg_cap),
        1usize << seg_min_cap
    ).next_power_of_two();
    let segment_size = (segment_size_raw / 30) * 30;
    let segment_size = segment_size.max(30);
    let num_segments = if xz == 0 { 1 } else { (xz / segment_size) + 1 };

    if num_segments <= 2 {
        return compute_d_serial(x, y, z, k, x_star, xz, primes, pi,
                                pi_sqrtz, pi_x_star,
                                segment_size, &template, &prime_recip,
                                valid_m_list);
    }

    // Work-balanced chunk assignment based on estimated Type 1 VM iterations per segment.
    let work_per_seg: Vec<usize> = (0..num_segments).map(|seg_idx| {
        let low = std::cmp::max(seg_idx * segment_size, 1) as u64;
        let high = std::cmp::min(low + segment_size as u64, xz as u64 + 1);
        let mut work = 0usize;
        // Estimate VM iterations by sampling a few b values across the Type 1 range
        let sample_bs = [8, 100, 300, 500, 700, 833];
        for &b in &sample_bs {
            if b >= primes.len() || b > pi_sqrtz { break; }
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let xp_low = std::cmp::min(x_div_prime / low, z as u64) as usize;
            let xp_high = std::cmp::min(x_div_prime / high, z as u64) as usize;
            let min_m = std::cmp::max(xp_high, z / prime as usize);
            let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);
            if max_m > min_m {
                work += max_m - min_m;
            }
        }
        // Also add base cost for cross-offs (proportional to number of b values)
        let cur_max_b = std::cmp::min(
            pi.get(std::cmp::min(isqrt(x / low) as usize, pi_limit)) as usize,
            pi_x_star);
        work += cur_max_b * 10; // base cross-off cost
        // Estimate Type 2 work: for sampled b in [pi_sqrtz+1, pi_x_star], count l-range
        let t2_samples = [pi_sqrtz + 1, (pi_sqrtz + pi_x_star) / 2, pi_x_star];
        for &b in &t2_samples {
            if b >= primes.len() || b > pi_x_star || b <= pi_sqrtz { continue; }
            let prime = primes[b] as u64;
            let x_div_prime = x / prime;
            let xp_low = std::cmp::min((x_div_prime / low) as usize, y);
            let xp_high = std::cmp::min((x_div_prime / high) as usize, y);
            let min_m = std::cmp::max(xp_high, prime as usize);
            let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);
            if max_m > min_m {
                let l_top = pi.get(std::cmp::min(max_m, pi_limit)) as usize;
                let l_bot = pi.get(std::cmp::min(min_m, pi_limit)) as usize;
                if l_top > l_bot { work += (l_top - l_bot) * 3; }
            }
        }
        std::cmp::max(work, 1)
    }).collect();

    let d_chunk_mult: usize = std::env::var("D_CHUNKS").ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| get_runtime_tuning().map(|t| t.d_chunks))
        .unwrap_or(24);
    let auto_chunk_select = std::env::var("D_AUTO_CHUNK_SELECT").ok()
        .map(|v| v != "0")
        .unwrap_or(false);
    let adapt_chunks = std::env::var("D_ADAPT_CHUNKS").ok()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|v| v != 0)
        .or_else(|| get_runtime_tuning().map(|t| t.d_adapt_chunks))
        .unwrap_or(false);
    let collect_vm_stats = std::env::var("D_VM_STATS").ok()
        .map(|v| v != "0")
        .unwrap_or(false);
    let show_timing = std::env::var("SHOW_TIMING").is_ok();
    let total_work: usize = work_per_seg.iter().sum();
    let avg_work = (total_work as f64) / (num_segments as f64);
    let max_work = *work_per_seg.iter().max().unwrap_or(&1) as f64;
    // Estimate p95 from a fixed-size sample to reduce scheduler overhead/noise.
    let sample_target = 4096usize;
    let stride = std::cmp::max(1, work_per_seg.len() / sample_target);
    let mut work_sample: Vec<usize> = work_per_seg.iter().step_by(stride).copied().collect();
    work_sample.sort_unstable();
    let p95_idx = ((work_sample.len() as f64) * 0.95) as usize;
    let p95_work = work_sample[std::cmp::min(p95_idx, work_sample.len() - 1)] as f64;
    let skew95 = if avg_work > 0.0 { p95_work / avg_work } else { 1.0 };
    let skew_max = if avg_work > 0.0 { max_work / avg_work } else { 1.0 };
    let nthreads = rayon::current_num_threads();
    let mut eff_chunk_mult: usize = if adapt_chunks {
        if skew95 >= 3.5 { 24 }
        else if skew95 <= 1.4 { 12 }
        else { d_chunk_mult }
    } else {
        d_chunk_mult
    };
    if auto_chunk_select {
        let candidates: [usize; 6] = [12, 16, 20, 24, 28, 32];
        let mut best = eff_chunk_mult;
        let mut best_obj = estimate_chunk_objective(&work_per_seg, nthreads, best);
        for &cand in &candidates {
            let obj = estimate_chunk_objective(&work_per_seg, nthreads, cand);
            if obj < best_obj {
                best_obj = obj;
                best = cand;
            }
        }
        eff_chunk_mult = best;
    }
    if show_timing {
        eprintln!(
            "    D chunking: base={} adapt={} auto={} eff={} skew95={:.2} skewMax={:.2} segs={}",
            d_chunk_mult, adapt_chunks as u8, auto_chunk_select as u8, eff_chunk_mult, skew95, skew_max, num_segments
        );
    }
    let nchunks = std::cmp::min(num_segments, nthreads * eff_chunk_mult);
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

    let results: Vec<(i64, Vec<i64>, Vec<i64>, usize, DVmStats)> = (0..actual_nchunks).into_par_iter().map(|tid| {
        let seg_start = chunk_bounds[tid];
        let seg_end = chunk_bounds[tid + 1];

        // Pre-compute max_b for this chunk to size phi/coeff vectors
        let chunk_max_b = (seg_start..seg_end).map(|seg_idx| {
            let low = std::cmp::max(seg_idx * segment_size, 1);
            if low > xz { return 0; }
            std::cmp::min(
                pi.get(std::cmp::min(isqrt(x / low as u64) as usize, pi_limit)) as usize,
                pi_x_star)
        }).max().unwrap_or(0);
        let vec_size = std::cmp::min(chunk_max_b + 1, nprimes);

        let max_wheel_bits = wheel_bit_count(segment_size);
        let mut sieve = BitSieve::new(max_wheel_bits);
        let mut phi = vec![0i64; vec_size];
        let mut d_local = 0i64;
        let mut coeff = vec![0i64; vec_size];
        let mut max_b_seen: usize = 0;
        let mut vm_stats = if collect_vm_stats {
            Some(DVmStats::default())
        } else {
            None
        };

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
                pi.get(std::cmp::min(isqrt(x / low1 as u64) as usize, pi_limit)) as usize,
                pi_x_star);
            if cur_max_b > max_b_seen { max_b_seen = cur_max_b; }

            let mut b = c + 1;

            // Type 1: b ≤ π(√z), squarefree m leaves (using precomputed ValidM list)
            while b <= std::cmp::min(pi_sqrtz, cur_max_b) && b < nprimes {
                let prime = primes[b] as u64;
                let prime_usize = prime as usize;
                let prime_u16 = prime as u16;
                let x_div_prime = x / prime;
                let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, z);
                let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, z);
                let min_m = std::cmp::max(xp_high, z / prime as usize);
                let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);

                if prime_usize >= max_m { break; }

                if min_m < max_m {
                    // Use sparse index for fast initial lookup, then short binary search
                    let min_bucket = std::cmp::min(min_m / vm_stride, vm_index.len() - 1);
                    let max_bucket = std::cmp::min(max_m / vm_stride, vm_index.len() - 1);
                    let bucket_delta = max_bucket - min_bucket;
                    let (vm_start, vm_end) = if min_bucket == max_bucket {
                        let hint = vm_index[min_bucket] as usize;
                        let search_end = if min_bucket + vm_lookahead < vm_index.len() {
                            vm_index[min_bucket + vm_lookahead] as usize
                        } else { valid_m_list.len() };
                        let search_end = std::cmp::min(search_end, valid_m_list.len());
                        let slice = &valid_m_list[hint..search_end];
                        let vm_start = hint + slice.partition_point(|v| (v.m as usize) <= min_m);
                        let vm_end = hint + slice.partition_point(|v| (v.m as usize) <= max_m);
                        if let Some(stats) = vm_stats.as_mut() {
                            stats.record_same_bucket(
                                bucket_delta,
                                search_end.saturating_sub(hint),
                                vm_end.saturating_sub(vm_start),
                            );
                        }
                        (vm_start, vm_end)
                    } else {
                        let vm_start = {
                            let hint = vm_index[min_bucket] as usize;
                            let search_end = if min_bucket + vm_lookahead < vm_index.len() {
                                vm_index[min_bucket + vm_lookahead] as usize
                            } else { valid_m_list.len() };
                            let search_end = std::cmp::min(search_end, valid_m_list.len());
                            let slice = &valid_m_list[hint..search_end];
                            hint + slice.partition_point(|v| (v.m as usize) <= min_m)
                        };
                        let start_hint = vm_index[min_bucket] as usize;
                        let start_search_end = if min_bucket + vm_lookahead < vm_index.len() {
                            vm_index[min_bucket + vm_lookahead] as usize
                        } else { valid_m_list.len() };
                        let start_search_end = std::cmp::min(start_search_end, valid_m_list.len());
                        let vm_end = {
                            let hint = vm_index[max_bucket] as usize;
                            let search_end = if max_bucket + vm_lookahead < vm_index.len() {
                                vm_index[max_bucket + vm_lookahead] as usize
                            } else { valid_m_list.len() };
                            let search_end = std::cmp::min(search_end, valid_m_list.len());
                            let slice = &valid_m_list[hint..search_end];
                            hint + slice.partition_point(|v| (v.m as usize) <= max_m)
                        };
                        let end_hint = vm_index[max_bucket] as usize;
                        let end_search_end = if max_bucket + vm_lookahead < vm_index.len() {
                            vm_index[max_bucket + vm_lookahead] as usize
                        } else { valid_m_list.len() };
                        let end_search_end = std::cmp::min(end_search_end, valid_m_list.len());
                        if let Some(stats) = vm_stats.as_mut() {
                            stats.record_cross_bucket(
                                bucket_delta,
                                start_search_end.saturating_sub(start_hint),
                                end_search_end.saturating_sub(end_hint),
                                vm_end.saturating_sub(vm_start),
                            );
                        }
                        (vm_start, vm_end)
                    };

                    let phi_b = phi[b];
                    {
                        let coeff_b = &mut coeff[b];
                        let mut prev_pos: Option<usize> = None;
                        let mut running_count: i64 = 0;
                        for v in valid_m_list[vm_start..vm_end].iter().rev() {
                            if prime_u16 < v.lpf {
                                let xpm = fast_div(x_div_prime, v.m as u64,
                                    v.recip_m) as usize;
                                // The vm window guarantees low <= xpm < high here.
                                if xpm != low {
                                    let pos = num_to_wheel_pos(xpm, low);
                                    let count = match prev_pos {
                                        None => { running_count = sieve.count(pos); running_count }
                                        Some(pp) if pos == pp => running_count,
                                        Some(pp) => { running_count += sieve.count_delta(pp, pos); running_count }
                                    };
                                    d_local -= v.mu_val as i64 * (phi_b + count);
                                    *coeff_b -= v.mu_val as i64;
                                    prev_pos = Some(pos);
                                } else if xpm == low {
                                    d_local -= v.mu_val as i64 * phi_b;
                                    *coeff_b -= v.mu_val as i64;
                                }
                            }
                        }
                    }
                }

                phi[b] += sieve.count_total();
                cross_off_sieve(&mut sieve, prime_usize, low, high, wheel_seg_bits);
                b += 1;
            }

            // Type 2: π(√z) < b ≤ π(x*), prime pair leaves
            while b <= cur_max_b && b < nprimes {
                let prime = primes[b] as u64;
                let prime_usize = prime as usize;
                let x_div_prime = x / prime;
                let xp_low = std::cmp::min((x_div_prime / low1 as u64) as usize, y);
                let xp_high = std::cmp::min((x_div_prime / high as u64) as usize, y);
                let min_m = std::cmp::max(xp_high, prime_usize);
                let max_m = std::cmp::min((x_div_prime / (prime * prime)) as usize, xp_low);
                let mut l = pi.get(std::cmp::min(max_m, pi_limit)) as usize;

                if l < nprimes && prime_usize >= primes[l] as usize { break; }

                let phi_b = phi[b];
                {
                    let coeff_b = &mut coeff[b];
                    let mut prev_pos: Option<usize> = None;
                    let mut running_count: i64 = 0;
                    while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                        let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                        // The l-window guarantees low <= xpq < high here.
                        if xpq != low {
                            let pos = num_to_wheel_pos(xpq, low);
                            let count = match prev_pos {
                                None => { running_count = sieve.count(pos); running_count }
                                Some(pp) if pos == pp => running_count,
                                Some(pp) => {
                                    running_count += sieve.count_delta(pp, pos);
                                    running_count
                                }
                            };
                            d_local += phi_b + count;
                            *coeff_b += 1;
                            prev_pos = Some(pos);
                        } else {
                            d_local += phi_b;
                            *coeff_b += 1;
                        }
                        l -= 1;
                    }
                }

                phi[b] += sieve.count_total();
                cross_off_sieve(&mut sieve, prime_usize, low, high, wheel_seg_bits);
                b += 1;
            }
        }

        (d_local, phi, coeff, max_b_seen, vm_stats.unwrap_or_default())
    }).collect();

    // Correction pass for phi offsets across chunk boundaries
    let mut d = results[0].0;
    let mut prefix_phi = results[0].1.clone();

    for kk in 1..results.len() {
        let (d_local, ref phi_total, ref coeff, max_b_seen, _) = results[kk];
        let limit = std::cmp::min(max_b_seen + 1, nprimes);
        let mut correction = 0i64;
        for bb in 0..limit {
            correction += prefix_phi[bb] * coeff[bb];
            prefix_phi[bb] += phi_total[bb];
        }
        d += d_local + correction;
    }

    if collect_vm_stats {
        let mut total_vm_stats = DVmStats::default();
        for (_, _, _, _, stats) in &results {
            total_vm_stats.merge(stats);
        }
        total_vm_stats.print(num_segments);
    }

    d
}

fn compute_d_serial(x: u64, y: usize, z: usize, k: usize, _x_star: usize, xz: usize,
                    primes: &[u32], pi: &CompactPi,
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
            pi.get(std::cmp::min(isqrt(x / low1 as u64) as usize, pi_limit)) as usize,
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
                        if xpm != low {
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
            let mut l = pi.get(std::cmp::min(max_m, pi_limit)) as usize;

            if l < nprimes && prime as usize >= primes[l] as usize { break; }

            let mut prev_pos: Option<usize> = None;
            let mut running_count: i64 = 0;
            while l > 0 && l < nprimes && (primes[l] as usize) > min_m {
                let xpq = fast_div(x_div_prime, primes[l] as u64, prime_recip[l]) as usize;
                if xpq != low {
                    let pos = num_to_wheel_pos(xpq, low);
                    let count = match prev_pos {
                        None => { running_count = sieve.count(pos); running_count }
                        Some(pp) if pos == pp => running_count,
                        Some(pp) => {
                            running_count += sieve.count_delta(pp, pos);
                            running_count
                        }
                    };
                    d += phi[b] + count;
                    prev_pos = Some(pos);
                } else {
                    d += phi[b];
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

    let auto_tune_enabled = std::env::var("AUTO_TUNE").ok()
        .map(|v| v != "0")
        .unwrap_or(false);
    if auto_tune_enabled {
        set_runtime_tuning(Some(choose_runtime_tuning(x)));
    } else {
        set_runtime_tuning(None);
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

    // Create thread pools early (before setup, needed for early AC/B start)
    let c1_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    let b_threads: usize = std::env::var("B_THREADS").ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(24));
    let b_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(b_threads)
        .build()
        .unwrap();

    let seq_mode = std::env::var("SEQ_MODE").is_ok();
    let special_mode = seq_mode
        || std::env::var("PHASE_DB_AC").is_ok()
        || std::env::var("PHASE_AC_DB").is_ok()
        || std::env::var("PHASE_D_ACB").is_ok()
        || std::env::var("PHASE_D_ACB2").is_ok()
        || std::env::var("PHASE_D_ACB3").is_ok()
        || std::env::var("PHASE_B_ACD").is_ok();

    if !special_mode {
        // BigPiTable needed for AC, B and Sigma.
        let t_setup = Instant::now();

        // gen_tables on detached OS thread (z, y are Copy)
        let gen_z = z;
        let gen_y = y;
        let gen_show = show_timing;
        let tables_handle = std::thread::spawn(move || {
            let t = Instant::now();
            let r = generate_tables(gen_z, gen_y);
            if gen_show { eprintln!("    gen_tables: {:.3}s", t.elapsed().as_secs_f64()); }
            r
        });

        // Scope 1: BigPiTable + main_setup (concurrent)
        let (big_pi, primes, pi_y, k, phi_cache, pi, recip) = std::thread::scope(|s| {
            let bpi_handle = s.spawn(|| { let t = Instant::now(); let r = BigPiTable::new(sqrt_x); if show_timing { eprintln!("    BigPiTable: {:.3}s", t.elapsed().as_secs_f64()); } r });

            let t_main = Instant::now();
            let sieve_bits = fast_bit_sieve(pi_table_limit);
            let primes = collect_primes_from_bits(&sieve_bits, y);
            let pi_y = primes.len() - 1;
            let k = std::cmp::min(7, pi_y);
            let phi_cache = PhiTinyCache::new(k);
            let pi = generate_compact_pi(pi_table_limit, &sieve_bits);
            drop(sieve_bits);
            let recip: Vec<u64> = primes.iter().map(|&p| {
                if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
            }).collect();
            if show_timing { eprintln!("    main_setup: {:.3}s", t_main.elapsed().as_secs_f64()); }

            let big_pi = bpi_handle.join().unwrap();

            (big_pi, primes, pi_y, k, phi_cache, pi, recip)
        });
        if show_timing { eprintln!("  AC/B/C1 start: {:.3}s", t_setup.elapsed().as_secs_f64()); }

        // Scope 2: AC / B / C1 concurrent; main → D
        // AC uses a dedicated rayon pool to isolate it from D's heavy tasks
        let ac_threads: usize = std::env::var("AC_THREADS").ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0); // 0 = use global pool (default)
        let ac_pool_opt = if ac_threads > 0 {
            Some(rayon::ThreadPoolBuilder::new()
                .num_threads(ac_threads)
                .build()
                .unwrap())
        } else {
            None
        };

        return std::thread::scope(|s| {
            let ac_done = Arc::new(AtomicBool::new(false));
            let ac_done_worker = ac_done.clone();
            let primes_ref = &primes;
            let pi_ref = &pi;
            let big_pi_ref = &big_pi;
            let recip_ref = &recip;
            let ac_handle = s.spawn(move || {
                let t = Instant::now();
                let r = if let Some(ref pool) = ac_pool_opt {
                    pool.install(|| compute_ac(x, y, z, k, x_star, primes_ref, pi_ref, big_pi_ref, recip_ref))
                } else {
                    compute_ac(x, y, z, k, x_star, primes_ref, pi_ref, big_pi_ref, recip_ref)
                };
                ac_done_worker.store(true, Ordering::Release);
                if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
                r
            });
            let b_handle = s.spawn(|| {
                let t = Instant::now();
                let r = b_pool.install(|| compute_b(x, y, pi_y, &big_pi));
                if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                r
            });
            let c1_handle = s.spawn(|| {
                c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi))
            });

            // Main thread: sigma + phi0 (overlaps with early AC/B)
            let sigma = compute_sigma(x, y, x_star, &primes, &pi, &big_pi);
            let phi0 = compute_phi0(x, y, z, k, &primes, &phi_cache);

            // Main thread: finish gen_tables + build_vm (D's prerequisites)
            let (mu, lpf, y_smooth) = tables_handle.join().unwrap();
            let t_vm = Instant::now();
            let (valid_m_list, vm_index) = build_valid_m(z, k, &primes, &pi, &mu, &lpf, &y_smooth);
            drop(mu); drop(lpf); drop(y_smooth);
            if show_timing { eprintln!("    build_vm:   {:.3}s (D starts at {:.3}s)", t_vm.elapsed().as_secs_f64(), t_setup.elapsed().as_secs_f64()); }

            // V10 scheduler: delay D start briefly to give AC uncontended DRAM bandwidth
            // during its high-intensity startup window. If AC finishes early, start D immediately.
            let d_wait_ms: u64 = std::env::var("D_WAIT_MS").ok()
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            if d_wait_ms > 0 {
                let t_wait = Instant::now();
                let deadline = t_wait + Duration::from_millis(d_wait_ms);
                while Instant::now() < deadline {
                    if ac_done.load(Ordering::Acquire) { break; }
                    std::thread::sleep(Duration::from_millis(1));
                }
                if show_timing {
                    eprintln!("    D wait:     {:.3}s", t_wait.elapsed().as_secs_f64());
                }
            }

            // D on dedicated pool (isolates D's heavy tasks from AC in global pool)
            let d_threads: usize = std::env::var("D_THREADS").ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0); // 0 = global pool (default)
            let t = Instant::now();
            let d = if d_threads > 0 {
                let d_pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(d_threads)
                    .build()
                    .unwrap();
                d_pool.install(|| compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip))
            } else {
                compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip)
            };
            if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }

            let ac = ac_handle.join().unwrap();
            let b_val = b_handle.join().unwrap();
            let c1 = c1_handle.join().unwrap();

            (c1 + ac - b_val + d + phi0 + sigma) as u64
        });
    }

    // Special modes: traditional setup → concurrent flow
    let t_setup = Instant::now();
    let (big_pi, primes, pi_y, k, phi_cache, pi, valid_m_list, vm_index) = std::thread::scope(|s| {
        let bpi_handle = s.spawn(|| { let t = Instant::now(); let r = BigPiTable::new(sqrt_x); if show_timing { eprintln!("    BigPiTable: {:.3}s", t.elapsed().as_secs_f64()); } r });
        let tables_handle = s.spawn(|| { let t = Instant::now(); let r = generate_tables(z, y); if show_timing { eprintln!("    gen_tables: {:.3}s", t.elapsed().as_secs_f64()); } r });

        let t_main = Instant::now();
        let sieve_bits = fast_bit_sieve(pi_table_limit);
        let primes = collect_primes_from_bits(&sieve_bits, y);
        let pi_y = primes.len() - 1;
        let k = std::cmp::min(7, pi_y);
        let phi_cache = PhiTinyCache::new(k);
        let pi = generate_compact_pi(pi_table_limit, &sieve_bits);
        drop(sieve_bits);
        if show_timing { eprintln!("    main_setup: {:.3}s", t_main.elapsed().as_secs_f64()); }

        let big_pi = bpi_handle.join().unwrap();
        let (mu, lpf, y_smooth) = tables_handle.join().unwrap();

        let t_vm = Instant::now();
        let (valid_m_list, vm_index) = build_valid_m(z, k, &primes, &pi, &mu, &lpf, &y_smooth);
        drop(mu); drop(lpf); drop(y_smooth);
        if show_timing { eprintln!("    build_vm:   {:.3}s", t_vm.elapsed().as_secs_f64()); }

        (big_pi, primes, pi_y, k, phi_cache, pi, valid_m_list, vm_index)
    });
    if show_timing { eprintln!("  setup tables: {:.2}s", t_setup.elapsed().as_secs_f64()); }

    let recip: Vec<u64> = primes.iter().map(|&p| {
        if p == 0 { 0 } else { ((1u128 << 64) / p as u128) as u64 }
    }).collect();

    let sigma = compute_sigma(x, y, x_star, &primes, &pi, &big_pi);
    let phi0 = compute_phi0(x, y, z, k, &primes, &phi_cache);

    let (ac, d, b_val, c1) = if seq_mode {
        let c1 = c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi));
        let t = Instant::now();
        let ac = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
        if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
        let t = Instant::now();
        let d = compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip);
        if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
        let t = Instant::now();
        // B on global pool in SEQ_MODE to give it all 72 threads
        let b_val = compute_b(x, y, pi_y, &big_pi);
        if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
        (ac, d, b_val, c1)
    } else if std::env::var("PHASE_DB_AC").is_ok() {
        let c1 = c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi));
        // Phase 1: D + B concurrent (no AC competing for BigPiTable bandwidth)
        let (d, b_val) = std::thread::scope(|s| {
            let b_handle = s.spawn(|| {
                let t = Instant::now();
                let r = b_pool.install(|| compute_b(x, y, pi_y, &big_pi));
                if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                r
            });
            let t = Instant::now();
            let d = compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip);
            if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
            (d, b_handle.join().unwrap())
        });
        // Phase 2: AC alone (full bandwidth for BigPiTable lookups)
        let t = Instant::now();
        let ac = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
        if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
        (ac, d, b_val, c1)
    } else if std::env::var("PHASE_AC_DB").is_ok() {
        let c1 = c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi));
        // Phase 1: AC alone (full L3 for BigPiTable)
        let t = Instant::now();
        let ac = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
        if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
        // Phase 2: D + B concurrent (D is memory-bound, B is CPU-bound — they coexist)
        let (d, b_val) = std::thread::scope(|s| {
            let b_handle = s.spawn(|| {
                let t = Instant::now();
                let r = b_pool.install(|| compute_b(x, y, pi_y, &big_pi));
                if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                r
            });
            let t = Instant::now();
            let d = compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip);
            if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
            (d, b_handle.join().unwrap())
        });
        (ac, d, b_val, c1)
    } else if std::env::var("PHASE_D_ACB").is_ok() || std::env::var("PHASE_D_ACB2").is_ok() || std::env::var("PHASE_D_ACB3").is_ok() {
        let c1 = c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi));
        // Phase 1: D alone (full resources)
        let t = Instant::now();
        let d = compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip);
        if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
        // Phase 2: AC + B concurrent
        if std::env::var("PHASE_D_ACB3").is_ok() {
            // AC on small dedicated pool (memory-bound, 8 threads sufficient)
            // B on global pool (CPU-bound, needs many threads)
            let ac_threads: usize = std::env::var("AC_THREADS").ok()
                .and_then(|s| s.parse().ok()).unwrap_or(8);
            let ac_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(ac_threads)
                .build()
                .unwrap();
            std::thread::scope(|s| {
                let b_handle = s.spawn(|| {
                    let t = Instant::now();
                    let r = compute_b(x, y, pi_y, &big_pi);
                    if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                    r
                });
                let t = Instant::now();
                let ac = ac_pool.install(|| compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip));
                if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
                let b_val = b_handle.join().unwrap();
                (ac, d, b_val, c1)
            })
        } else if std::env::var("PHASE_D_ACB2").is_ok() {
            // Both AC and B on global pool (cache-friendly: both access BigPiTable)
            std::thread::scope(|s| {
                let b_handle = s.spawn(|| {
                    let t = Instant::now();
                    let r = compute_b(x, y, pi_y, &big_pi);
                    if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                    r
                });
                let t = Instant::now();
                let ac = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
                if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
                let b_val = b_handle.join().unwrap();
                (ac, d, b_val, c1)
            })
        } else {
            std::thread::scope(|s| {
                let b_handle = s.spawn(|| {
                    let t = Instant::now();
                    let r = b_pool.install(|| compute_b(x, y, pi_y, &big_pi));
                    if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
                    r
                });
                let t = Instant::now();
                let ac = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
                if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
                let b_val = b_handle.join().unwrap();
                (ac, d, b_val, c1)
            })
        }
    } else if std::env::var("PHASE_B_ACD").is_ok() {
        let c1 = c1_pool.install(|| compute_c1(x, y, z, k, &primes, &pi));
        // Phase 1: B alone (fast ~2.8s, full L3 and all cores)
        let t = Instant::now();
        let b_val = b_pool.install(|| compute_b(x, y, pi_y, &big_pi));
        if show_timing { eprintln!("  B: {:.2}s", t.elapsed().as_secs_f64()); }
        // Phase 2: AC + D concurrent (no B competing for cores/memory)
        let (ac, d) = std::thread::scope(|s| {
            let ac_handle = s.spawn(|| {
                let t = Instant::now();
                let r = compute_ac(x, y, z, k, x_star, &primes, &pi, &big_pi, &recip);
                if show_timing { eprintln!("  AC: {:.2}s", t.elapsed().as_secs_f64()); }
                r
            });
            let t = Instant::now();
            let d = compute_d(x, y, z, k, x_star, &primes, &pi, &valid_m_list, &vm_index, &recip);
            if show_timing { eprintln!("  D: {:.2}s", t.elapsed().as_secs_f64()); }
            let ac = ac_handle.join().unwrap();
            (ac, d)
        });
        (ac, d, b_val, c1)
    } else {
        unreachable!("special_mode check should have caught this")
    };

    (c1 + ac - b_val + d + phi0 + sigma) as u64
}

fn main() {
    // Set high process priority for consistent performance
    #[cfg(target_os = "windows")]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn SetPriorityClass(hProcess: *mut std::ffi::c_void, dwPriorityClass: u32) -> i32;
        }
        const HIGH_PRIORITY_CLASS: u32 = 0x00000080;
        SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }

    // Enable large pages: activate SeLockMemoryPrivilege and tell mimalloc to use 2MB pages
    #[cfg(target_os = "windows")]
    {
        large_page_alloc::enable_large_pages();
    }

    // Oversubscribe rayon thread pool for better work-stealing across B/AC/D
    let num_cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(24);
    let pool_mult: usize = std::env::var("POOL_MULT").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(3);
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_cpus * pool_mult)
        .build_global()
        .ok();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Prime Counter V10 — Gourdon's Algorithm                   ║");
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
