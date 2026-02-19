// prime_display.rs
//
// V1 segmented sieve (wheel-30, pre-sieve {7,11,13,17,19}, DK_TABLE,
// Barrett reciprocals, L1 carry-forward tiny primes) adapted for continuous,
// ordered prime output with a live TUI display matching the Pi Sieve style.
//
// ── Display layout ────────────────────────────────────────────────────────────
//
//  ┌──────────────────────────────────────────────────────┐  ← blue title bar
//  │  Pi Sieve — High Speed Prime Generation              │
//  │  Primes: 225,698,304    [green]                      │
//  │  Runtime:   0 years 0 days 11:02:32                  │
//  │  Remaining: ∞  (or time estimate if --limit given)   │
//  │  Speed:   ~50,847,534 primes/sec                     │
//  ├──────────────────────────────────────────────────────┤  ← separator
//  │  4,791,888,565                                       │  ↑
//  │  4,791,888,591                                       │  scrolling
//  │  4,791,888,609                                       │  prime list
//  │  ...                                                 │  ↓
//  └──────────────────────────────────────────────────────┘
//
// ── Architecture ─────────────────────────────────────────────────────────────
//
// The sieve runs sequentially (no Rayon) so primes emerge in sorted order.
// A scroll region is set to lines [HEADER_LINES+1 .. terminal_height] using
// the DECSTBM VT100 escape.  The header is redrawn on a fixed timer (~10 Hz)
// by: saving cursor, moving to row 1, printing 6 lines, restoring cursor.
//
// Primes are written to a BufWriter<Stdout> in batches for throughput; the
// write format is a single formatted line per prime pushed to the scroll
// region's bottom row, which causes the region to scroll up naturally.
//
// All V1 single-threaded optimizations are intact:
//   • Wheel mod 30  (8 candidates / 30 integers per sieve byte)
//   • Pre-sieve 7×11×13×17×19 = 323 323-byte tiling pattern
//   • DK_TABLE for branch-free compute_starts
//   • Barrett reciprocal fast division  (ceil(2^64/p) → u128 mulhi)
//   • L1 sub-segmentation with carry-forward starts for tiny primes
//
// Usage:
//   prime_display                     # run to u64::MAX (years of runtime)
//   prime_display --limit 1000000000  # stop after 10^9

use primal::Sieve;
use std::io::{self, BufWriter, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crossterm::{
    cursor,
    execute, queue,
    style::{Color, Print, SetBackgroundColor, SetForegroundColor, ResetColor, Attribute, SetAttribute},
    terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

// ── Wheel mod 30 ─────────────────────────────────────────────────────────────
const OFFSETS: [u32; 8] = [1, 7, 11, 13, 17, 19, 23, 29];
const BITS:    [u8;  8] = [1, 2,  4,  8, 16, 32, 64, 128];

// ── Segment sizing ───────────────────────────────────────────────────────────
const MAX_SEG_BYTES: usize = 1024 * 1024; // 1 MB
const MIN_SEG_BYTES: usize = 8    * 1024; // 8 KB  (unused in sequential mode but kept)
const L1_SEG_BYTES:  usize = 24   * 1024; // 24 KB (fits Arrow Lake L1d at ½)

// ── Pre-sieve ────────────────────────────────────────────────────────────────
const PRESIEVE_PRIMES: [u64; 5] = [7, 11, 13, 17, 19];
const PRESIEVE_PERIOD: usize    = 7 * 11 * 13 * 17 * 19; // 323 323

// ── Display constants ─────────────────────────────────────────────────────────
const HEADER_LINES: u16 = 7; // title + 5 stat lines + separator

// ── Compile-time wheel tables ─────────────────────────────────────────────────
#[inline(always)]
const fn mod_inv_30(a: u32) -> u32 {
    match a {
        1 => 1, 7 => 13, 11 => 11, 13 => 7,
        17 => 23, 19 => 19, 23 => 17, 29 => 29, _ => 1,
    }
}

const TARGET_K_MOD: [[u64; 8]; 8] = {
    let mut t = [[0u64; 8]; 8];
    let off = [1u64, 7, 11, 13, 17, 19, 23, 29];
    let mut pi = 0;
    while pi < 8 {
        let inv = mod_inv_30(off[pi] as u32) as u64;
        let mut oi = 0;
        while oi < 8 { t[pi][oi] = (off[oi] * inv) % 30; oi += 1; }
        pi += 1;
    }
    t
};

const P_MOD_TO_IDX: [u8; 30] = {
    let mut t = [0u8; 30];
    t[1]=0; t[7]=1; t[11]=2; t[13]=3; t[17]=4; t[19]=5; t[23]=6; t[29]=7;
    t
};

const DK_TABLE: [[[u8; 8]; 30]; 8] = {
    let mut t = [[[0u8; 8]; 30]; 8];
    let mut pi = 0;
    while pi < 8 {
        let mut kr = 0u64;
        while kr < 30 {
            let mut i = 0;
            while i < 8 {
                let tgt = TARGET_K_MOD[pi][i];
                t[pi][kr as usize][i] = (if kr <= tgt { tgt - kr } else { 30 - kr + tgt }) as u8;
                i += 1;
            }
            kr += 1;
        }
        pi += 1;
    }
    t
};

// ── Sieving prime descriptor ──────────────────────────────────────────────────
struct SievePrime {
    p:     u32,
    p_idx: u8,
    recip: u64, // ceil(2^64 / p)
}

fn precompute_sieve_primes(primes: &[u32]) -> Vec<SievePrime> {
    primes.iter().map(|&p| SievePrime {
        p,
        p_idx: P_MOD_TO_IDX[(p % 30) as usize],
        recip: u64::MAX / p as u64 + 1,
    }).collect()
}

#[inline(always)]
fn fast_div(n: u64, recip: u64) -> u64 {
    ((n as u128 * recip as u128) >> 64) as u64
}

// ── Pre-sieve pattern ────────────────────────────────────────────────────────
static PRESIEVE: OnceLock<Vec<u8>> = OnceLock::new();

fn build_presieve() -> Vec<u8> {
    let mut pat = vec![0u8; PRESIEVE_PERIOD];
    for &p in &PRESIEVE_PRIMES {
        let p_idx = P_MOD_TO_IDX[(p % 30) as usize] as usize;
        for i in 0..8 {
            let target = TARGET_K_MOD[p_idx][i];
            let mut idx = ((target * p) / 30) as usize;
            while idx < PRESIEVE_PERIOD { pat[idx] |= BITS[i]; idx += p as usize; }
        }
    }
    pat
}

fn get_presieve() -> &'static [u8] { PRESIEVE.get_or_init(build_presieve) }

#[inline]
fn apply_presieve(sieve: &mut [u8], pat_off: usize, presieve: &[u8]) {
    let len = sieve.len();
    let mut dst = 0;
    let first = (PRESIEVE_PERIOD - pat_off).min(len);
    sieve[..first].copy_from_slice(&presieve[pat_off..pat_off + first]);
    dst += first;
    while dst + PRESIEVE_PERIOD <= len {
        sieve[dst..dst + PRESIEVE_PERIOD].copy_from_slice(presieve);
        dst += PRESIEVE_PERIOD;
    }
    if dst < len { sieve[dst..].copy_from_slice(&presieve[..len - dst]); }
}

// ── compute_starts (Barrett, DK_TABLE) ───────────────────────────────────────
#[inline]
fn compute_starts(sp: &SievePrime, seg_start: u64, seg_len: usize) -> [usize; 8] {
    let p = sp.p as u64;
    let mut s = [usize::MAX; 8];
    let k_min   = fast_div(seg_start + p - 1, sp.recip);
    let k_rem   = (k_min % 30) as usize;
    let base    = k_min * p - seg_start;
    let dks     = &DK_TABLE[sp.p_idx as usize][k_rem];
    for i in 0..8 {
        let b = ((base + dks[i] as u64 * p) / 30) as usize;
        if b < seg_len { s[i] = b; }
    }
    s
}

// ── Sieve marking ─────────────────────────────────────────────────────────────
#[inline]
unsafe fn sieve_tiny(sieve: &mut [u8], starts: &mut [u32; 8], p: usize, sub_off: usize) {
    let len = sieve.len();
    for i in 0..8 {
        let s = starts[i] as usize;
        if s < sub_off || s - sub_off >= len { continue; }
        let mut idx = s - sub_off;
        let bit = BITS[i];
        let end4 = len.saturating_sub(3 * p);
        while idx < end4 {
            *sieve.get_unchecked_mut(idx)         |= bit;
            *sieve.get_unchecked_mut(idx +     p) |= bit;
            *sieve.get_unchecked_mut(idx + 2 * p) |= bit;
            *sieve.get_unchecked_mut(idx + 3 * p) |= bit;
            idx += 4 * p;
        }
        while idx < len { *sieve.get_unchecked_mut(idx) |= bit; idx += p; }
        starts[i] = (sub_off + idx) as u32;
    }
}

#[inline]
unsafe fn sieve_medium(sieve: &mut [u8], starts: &[usize; 8], p: usize) {
    let len = sieve.len();
    for i in 0..8 {
        let mut idx = starts[i];
        if idx >= len { continue; }
        let bit = BITS[i];
        while idx < len { *sieve.get_unchecked_mut(idx) |= bit; idx += p; }
    }
}

#[inline]
unsafe fn sieve_large(sieve: &mut [u8], starts: &[usize; 8]) {
    let len = sieve.len();
    for i in 0..8 {
        let idx = starts[i];
        if idx < len { *sieve.get_unchecked_mut(idx) |= BITS[i]; }
    }
}

// ── Format helpers ────────────────────────────────────────────────────────────

/// Format a duration (seconds) as "Y years D days HH:MM:SS"
fn fmt_duration(mut secs: u64) -> String {
    const SECS_MIN:  u64 = 60;
    const SECS_HOUR: u64 = 3600;
    const SECS_DAY:  u64 = 86_400;
    const SECS_YEAR: u64 = 365 * SECS_DAY;

    let years = secs / SECS_YEAR;  secs %= SECS_YEAR;
    let days  = secs / SECS_DAY;   secs %= SECS_DAY;
    let hours = secs / SECS_HOUR;  secs %= SECS_HOUR;
    let mins  = secs / SECS_MIN;   secs %= SECS_MIN;
    format!("{} years {} days {:02}:{:02}:{:02}", years, days, hours, mins, secs)
}

/// Comma-separate a u64
fn comma(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let offset = s.len() % 3;
    for (i, c) in s.chars().enumerate() {
        if i > 0 && i >= offset && (i - offset) % 3 == 0 { out.push(','); }
        out.push(c);
    }
    out
}

// ── Terminal display ──────────────────────────────────────────────────────────

/// Set VT100 scrolling region to [top_row, bot_row] (1-indexed).
fn set_scroll_region(top: u16, bot: u16) -> io::Result<()> {
    // Raw DECSTBM escape: ESC [ Pt ; Pb r
    print!("\x1b[{};{}r", top, bot);
    Ok(())
}

/// Move cursor to an absolute row/col (1-indexed) using raw escape.
#[inline]
fn goto_raw(row: u16, col: u16) {
    print!("\x1b[{};{}H", row, col);
}

struct Display {
    term_rows: u16,
    term_cols: u16,
}

impl Display {
    fn new() -> io::Result<Self> {
        let (cols, rows) = terminal::size()?;
        Ok(Display { term_rows: rows, term_cols: cols })
    }

    fn init(&self) -> io::Result<()> {
        let mut stdout = io::stdout();
        execute!(stdout,
            EnterAlternateScreen,
            terminal::Clear(ClearType::All),
            cursor::Hide,
        )?;
        // Set scroll region to prime area (row HEADER_LINES+1 .. term_rows)
        set_scroll_region(HEADER_LINES + 1, self.term_rows);
        // Draw initial header
        self.draw_header(0, 0, 0, 0, None)?;
        // Position cursor at start of scrolling area (bottom row of scroll region)
        goto_raw(self.term_rows, 1);
        io::stdout().flush()?;
        Ok(())
    }

    fn cleanup(&self) -> io::Result<()> {
        let mut stdout = io::stdout();
        // Reset scroll region to full terminal
        set_scroll_region(1, self.term_rows);
        execute!(stdout,
            cursor::Show,
            LeaveAlternateScreen,
        )?;
        Ok(())
    }

    /// Redraw the fixed header (lines 1..=HEADER_LINES).
    /// Saves cursor, draws, restores cursor to scroll region bottom.
    fn draw_header(
        &self,
        prime_count: u64,
        elapsed_secs: u64,
        speed: u64,           // numbers/sec processed
        primes_per_sec: u64,  // primes/sec found
        limit: Option<u64>,   // None ⟹ ∞ mode
    ) -> io::Result<()> {
        let cols = self.term_cols as usize;
        let title = " Pi Sieve \u{2014} High Speed Prime Generation";

        // Save cursor, move to top
        print!("\x1b[s"); // save
        goto_raw(1, 1);

        // ── Line 1: title bar (blue background, white text) ───────────────────
        let title_padded = format!("{:<width$}", title, width = cols);
        print!("\x1b[44;97m{}\x1b[0m", title_padded); // blue bg, bright white

        // ── Line 2: prime count (cyan label, green value) ─────────────────────
        goto_raw(2, 1);
        print!("\x1b[0m\x1b[2K"); // clear line
        print!("  \x1b[36mPrimes:\x1b[0m  \x1b[92;1m{:<20}\x1b[0m", comma(prime_count));

        // ── Line 3: runtime ───────────────────────────────────────────────────
        goto_raw(3, 1);
        print!("\x1b[0m\x1b[2K");
        print!("  \x1b[36mRuntime:\x1b[0m   {}", fmt_duration(elapsed_secs));

        // ── Line 4: remaining ─────────────────────────────────────────────────
        goto_raw(4, 1);
        print!("\x1b[0m\x1b[2K");
        match limit {
            None => {
                print!("  \x1b[36mRemaining:\x1b[0m \x1b[90m\u{221e}\x1b[0m");
            }
            Some(lim) => {
                // Estimate based on speed (numbers/sec processed)
                if speed > 0 && prime_count > 0 {
                    // We can estimate: time_elapsed / prime_count * remaining_primes
                    // Approximate remaining primes ≈ lim/ln(lim) - prime_count
                    let est_total = (lim as f64 / (lim as f64).ln()) as u64;
                    let remaining_primes = est_total.saturating_sub(prime_count);
                    // primes/sec ≈ speed * (prime_count / numbers_processed_so_far)
                    // simpler: use elapsed_secs / prime_count rate
                    let remaining_secs = if elapsed_secs > 0 {
                        remaining_primes * elapsed_secs / prime_count.max(1)
                    } else { u64::MAX };
                    if remaining_secs < 400_000 * 365 * 86_400 {
                        print!("  \x1b[36mRemaining:\x1b[0m {}", fmt_duration(remaining_secs));
                    } else {
                        print!("  \x1b[36mRemaining:\x1b[0m \x1b[90m(very long)\x1b[0m");
                    }
                } else {
                    print!("  \x1b[36mRemaining:\x1b[0m \x1b[90m(calculating)\x1b[0m");
                }
            }
        }

        // ── Line 5: speed ─────────────────────────────────────────────────────
        goto_raw(5, 1);
        print!("\x1b[0m\x1b[2K");
        if speed > 0 {
            print!("  \x1b[36mSpeed:\x1b[0m     ~{} numbers/second", comma(speed));
        } else {
            print!("  \x1b[36mSpeed:\x1b[0m     (warming up)");
        }

        // ── Line 6: primes/sec ────────────────────────────────────────────────
        goto_raw(6, 1);
        print!("\x1b[0m\x1b[2K");
        if primes_per_sec > 0 {
            print!("  \x1b[36mPrimes/s:\x1b[0m  ~{} primes/second", comma(primes_per_sec));
        } else {
            print!("  \x1b[36mPrimes/s:\x1b[0m  (warming up)");
        }

        // ── Line 7: separator with exit hint ──────────────────────────────────
        goto_raw(7, 1);
        print!("\x1b[0m\x1b[2K");
        let hint = " Press q to exit ";
        let sep_len = cols.saturating_sub(hint.len());
        let left = sep_len / 2;
        let right = sep_len - left;
        print!("\x1b[90m{}\x1b[33m{}\x1b[90m{}\x1b[0m",
               "\u{2500}".repeat(left), hint, "\u{2500}".repeat(right));

        // Restore cursor to scroll region bottom (where next prime will be printed)
        print!("\x1b[u"); // restore
        Ok(())
    }
}

// ── Main sieve loop ───────────────────────────────────────────────────────────

fn run(limit: u64, stop: Arc<AtomicBool>) -> io::Result<()> {
    let disp = Display::new()?;
    terminal::enable_raw_mode()?;
    disp.init()?;

    let stdout_raw = io::stdout();
    // BufWriter: batch writes for throughput; flush at header-update intervals
    let mut out = BufWriter::with_capacity(256 * 1024, stdout_raw);

    // ── Snapshot rendering: buffer primes, render at 24 fps ─────────────────────
    let visible_rows = (disp.term_rows - HEADER_LINES) as usize;
    let mut recent_primes: VecDeque<u64> = VecDeque::with_capacity(visible_rows + 1);

    let t_start = Instant::now();
    let mut prime_count: u64 = 0;
    let mut last_frame = Instant::now();
    let mut speed_window_start = Instant::now();
    let mut numbers_in_window: u64 = 0;
    let mut speed: u64 = 0;
    let mut primes_per_sec: u64 = 0;
    let mut last_window_prime_count: u64 = 0;
    const FRAME_INTERVAL_MS: u128 = 42; // ~24 fps

    // Buffer a prime (used only for small/bootstrap primes — few calls)
    macro_rules! emit {
        ($n:expr) => {{
            recent_primes.push_back($n);
            if recent_primes.len() > visible_rows {
                recent_primes.pop_front();
            }
        }};
    }

    // Render one frame: header + visible primes
    macro_rules! render_frame {
        () => {{
            out.flush()?;
            let elapsed = t_start.elapsed().as_secs();
            let lim_opt = if limit == u64::MAX { None } else { Some(limit) };
            disp.draw_header(prime_count, elapsed, speed, primes_per_sec, lim_opt)?;
            for (i, &p) in recent_primes.iter().enumerate() {
                let row = HEADER_LINES + 1 + i as u16;
                write!(out, "\x1b[{};1H\x1b[2K\x1b[97m{:>24}\x1b[0m", row, comma(p))?;
            }
            out.flush()?;
        }};
    }

    // Extract the last visible_rows primes from a sieve segment (walks backward)
    macro_rules! extract_recent {
        ($sieve:expr, $seg_start:expr, $seg_len:expr, $lim:expr) => {{
            recent_primes.clear();
            let mut found = 0usize;
            'outer: for byte_idx in (0..$seg_len).rev() {
                let byte = $sieve[byte_idx];
                if byte == 0xFF { continue; }
                let base = $seg_start + byte_idx as u64 * 30;
                for bit_idx in (0..8usize).rev() {
                    if byte & BITS[bit_idx] == 0 {
                        let n = base + OFFSETS[bit_idx] as u64;
                        if n <= $lim {
                            recent_primes.push_front(n);
                            found += 1;
                            if found >= visible_rows { break 'outer; }
                        }
                    }
                }
            }
        }};
    }

    // Conditionally render if a frame interval has elapsed
    macro_rules! maybe_render {
        () => {{
            let now = Instant::now();
            if now.duration_since(last_frame).as_millis() >= FRAME_INTERVAL_MS {
                let window_secs = now.duration_since(speed_window_start).as_secs_f64();
                if window_secs >= 0.5 {
                    speed = (numbers_in_window as f64 / window_secs) as u64;
                    primes_per_sec = ((prime_count - last_window_prime_count) as f64 / window_secs) as u64;
                    numbers_in_window = 0;
                    last_window_prime_count = prime_count;
                    speed_window_start = now;
                }
                render_frame!();
                last_frame = now;
            }
        }};
    }

    // ── Handle 2, 3, 5 (wheel exclusions) ────────────────────────────────────
    for &p in &[2u64, 3, 5] {
        if p > limit { break; }
        prime_count += 1;
        emit!(p);
    }

    if limit < 7 {
        render_frame!();
        wait_for_key(&stop)?;
        disp.cleanup()?;
        terminal::disable_raw_mode()?;
        return Ok(());
    }

    // ── Bootstrap sieve ───────────────────────────────────────────────────────
    let sqrt_n      = ((limit as f64).sqrt() as usize) + 2;
    let small_sieve = Sieve::new(sqrt_n);

    let sieve_start: u64 = {
        let s = sqrt_n as u64 + 1;
        ((s + 29) / 30) * 30
    };

    // Emit small primes (7 ≤ p < sieve_start), skipping presieve primes for now
    // We'll do them inline via the bootstrap sieve
    for p in small_sieve.primes_from(7) {
        let p64 = p as u64;
        if p64 >= sieve_start || p64 > limit { break; }
        prime_count += 1;
        emit!(p64);
        if prime_count & 0x1FFF == 0 { maybe_render!(); }
    }

    if sieve_start > limit {
        render_frame!();
        wait_for_key(&stop)?;
        disp.cleanup()?;
        terminal::disable_raw_mode()?;
        return Ok(());
    }

    // ── Sieving primes (7..√N minus presieve primes) ─────────────────────────
    let sieving_primes_raw: Vec<u32> = small_sieve
        .primes_from(7)
        .filter(|&p| !PRESIEVE_PRIMES.contains(&(p as u64)))
        .map(|p| p as u32)
        .collect();
    let sieving_primes = precompute_sieve_primes(&sieving_primes_raw);

    // ── Segment sizing ────────────────────────────────────────────────────────
    let seg_bytes = MAX_SEG_BYTES; // single-threaded: always use max

    // ── Prime tier split ──────────────────────────────────────────────────────
    let tiny_threshold  = (L1_SEG_BYTES / 3) as u32;
    let large_threshold = seg_bytes as u32;
    let tiny_split  = sieving_primes.partition_point(|sp| sp.p < tiny_threshold);
    let large_split = sieving_primes.partition_point(|sp| sp.p < large_threshold);
    let tiny_primes  = &sieving_primes[..tiny_split];
    let small_primes = &sieving_primes[tiny_split..large_split];
    let large_primes = &sieving_primes[large_split..];

    // Carry-forward buffer for tiny prime starts across L1 sub-segments
    let mut tiny_starts: Vec<[u32; 8]> = vec![[0u32; 8]; tiny_primes.len()];

    // ── Segment buffer ────────────────────────────────────────────────────────
    let mut sieve_buf = vec![0u8; seg_bytes];

    let presieve         = get_presieve();
    let total_numbers    = limit - sieve_start + 1;
    let total_bytes      = ((total_numbers + 29) / 30) as usize;
    let presieve_base_off = ((sieve_start / 30) as usize) % PRESIEVE_PERIOD;

    let num_segs = (total_bytes + seg_bytes - 1) / seg_bytes;

    // ── Main sieve loop ───────────────────────────────────────────────────────
    let mut last_seg_start: u64 = sieve_start;
    let mut last_seg_len: usize = 0;
    'seg: for seg_idx in 0..num_segs {
        if stop.load(Ordering::Relaxed) { break 'seg; }

        let byte_offset    = seg_idx * seg_bytes;
        let seg_start_num  = sieve_start + byte_offset as u64 * 30;
        let seg_byte_count = (total_bytes - byte_offset).min(seg_bytes);
        let sieve          = &mut sieve_buf[..seg_byte_count];
        last_seg_start = seg_start_num;
        last_seg_len   = seg_byte_count;

        // ── Pre-sieve ─────────────────────────────────────────────────────────
        let pat_off = (presieve_base_off + byte_offset) % PRESIEVE_PERIOD;
        apply_presieve(sieve, pat_off, presieve);

        // ── Tiny primes: compute starts once, carry forward across L1 ─────────
        if !tiny_primes.is_empty() {
            for (pi, sp) in tiny_primes.iter().enumerate() {
                let s = compute_starts(sp, seg_start_num, seg_byte_count);
                for j in 0..8 {
                    tiny_starts[pi][j] = if s[j] == usize::MAX { u32::MAX } else { s[j] as u32 };
                }
            }
            let mut sub_off = 0usize;
            while sub_off < seg_byte_count {
                let sub_len = L1_SEG_BYTES.min(seg_byte_count - sub_off);
                let sub = &mut sieve[sub_off..sub_off + sub_len];
                for (pi, sp) in tiny_primes.iter().enumerate() {
                    unsafe { sieve_tiny(sub, &mut tiny_starts[pi], sp.p as usize, sub_off); }
                }
                sub_off += L1_SEG_BYTES;
            }
        }

        // ── Small / medium primes ─────────────────────────────────────────────
        for sp in small_primes {
            let s = compute_starts(sp, seg_start_num, seg_byte_count);
            unsafe { sieve_medium(sieve, &s, sp.p as usize); }
        }

        // ── Large primes ──────────────────────────────────────────────────────
        for sp in large_primes {
            let s = compute_starts(sp, seg_start_num, seg_byte_count);
            unsafe { sieve_large(sieve, &s); }
        }

        // ── Count primes via popcount (no per-prime overhead) ─────────────────
        {
            let mut seg_primes: u64 = 0;
            // Process 8 bytes at a time using 64-bit popcount
            let whole_u64s = seg_byte_count / 8;
            let sieve_ptr = sieve.as_ptr() as *const u64;
            for i in 0..whole_u64s {
                let word = unsafe { sieve_ptr.add(i).read_unaligned() };
                seg_primes += (!word).count_ones() as u64;
            }
            for byte_idx in (whole_u64s * 8)..seg_byte_count {
                seg_primes += (!sieve[byte_idx]).count_ones() as u64;
            }
            // Correct for candidates beyond limit in last segment's last byte
            if seg_idx == num_segs - 1 && seg_byte_count > 0 {
                let last_byte = sieve[seg_byte_count - 1];
                let base = seg_start_num + (seg_byte_count - 1) as u64 * 30;
                for bit_idx in 0..8 {
                    if last_byte & BITS[bit_idx] == 0 {
                        let n = base + OFFSETS[bit_idx] as u64;
                        if n > limit { seg_primes -= 1; }
                    }
                }
            }
            prime_count += seg_primes;
        }

        numbers_in_window += seg_byte_count as u64 * 30;

        // ── Render frame with lazy prime extraction ───────────────────────────
        {
            let now = Instant::now();
            if now.duration_since(last_frame).as_millis() >= FRAME_INTERVAL_MS {
                let window_secs = now.duration_since(speed_window_start).as_secs_f64();
                if window_secs >= 0.5 {
                    speed = (numbers_in_window as f64 / window_secs) as u64;
                    primes_per_sec = ((prime_count - last_window_prime_count) as f64 / window_secs) as u64;
                    numbers_in_window = 0;
                    last_window_prime_count = prime_count;
                    speed_window_start = now;
                }
                extract_recent!(sieve, seg_start_num, seg_byte_count, limit);
                render_frame!();
                last_frame = now;
            }
        }
    }

    // ── Final frame (extract from last segment's sieve data) ────────────────
    if last_seg_len > 0 {
        let sieve = &sieve_buf[..last_seg_len];
        extract_recent!(sieve, last_seg_start, last_seg_len, limit);
    }
    render_frame!();

    // Wait for keypress before exiting (so the user can read the screen)
    wait_for_key(&stop)?;
    disp.cleanup()?;
    terminal::disable_raw_mode()?;

    // Print summary to normal stdout after restoring terminal
    let total_elapsed = t_start.elapsed().as_secs();
    println!("\nTotal primes found: {}", comma(prime_count));
    println!("Runtime:            {}", fmt_duration(total_elapsed));

    Ok(())
}

// ── Wait for any keypress (or stop flag) ─────────────────────────────────────
fn wait_for_key(stop: &Arc<AtomicBool>) -> io::Result<()> {
    use crossterm::event::{self, Event, KeyCode};
    // move cursor to a visible row and show a prompt
    print!("\x1b[1;1H\x1b[43;30m  Press any key to exit  \x1b[0m");
    io::stdout().flush()?;
    loop {
        if stop.load(Ordering::Relaxed) { break; }
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(_) = event::read()? { break; }
        }
    }
    Ok(())
}

// ═════════════════════════════════════════════════════════════════════════════
// Entry point
// ═════════════════════════════════════════════════════════════════════════════

fn main() {
    // ── Parse --limit N argument ──────────────────────────────────────────────
    let limit: u64 = {
        let args: Vec<String> = std::env::args().collect();
        let mut lim = u64::MAX;
        let mut i = 1;
        while i < args.len() {
            if (args[i] == "--limit" || args[i] == "-l") && i + 1 < args.len() {
                lim = args[i + 1].replace('_', "").parse().unwrap_or(u64::MAX);
                i += 2;
            } else {
                eprintln!("Usage: prime_display [--limit N]");
                std::process::exit(1);
            }
        }
        lim
    };

    // ── Ctrl+C / SIGINT: set stop flag ────────────────────────────────────────
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop_clone = Arc::clone(&stop);
        // Restore terminal if we panic
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
            default_hook(info);
        }));
        ctrlc_install(stop_clone);
    }

    if let Err(e) = run(limit, stop) {
        // Make sure we restore terminal state on any error
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ── Portable Ctrl+C handler via a background thread ──────────────────────────
fn ctrlc_install(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        use crossterm::event::{self, Event, KeyCode, KeyModifiers};
        loop {
            if let Ok(true) = event::poll(std::time::Duration::from_millis(50)) {
                if let Ok(Event::Key(k)) = event::read() {
                    // Ctrl+C or 'q'
                    if k.code == KeyCode::Char('c') && k.modifiers == crossterm::event::KeyModifiers::CONTROL {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    if k.code == KeyCode::Char('q') {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    });
}