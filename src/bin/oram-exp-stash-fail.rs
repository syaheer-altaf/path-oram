use oram::path_oram::{
    DEFAULT_BLOCKS_PER_BUCKET, DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF,
    DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{
    Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff, StashSize,
};

use rand::rngs::OsRng;
use rand::RngCore;
use std::fs;
use std::io::Write;
use std::time::Instant;

/* ── ORAM parameters ──────────────────────────────────────────────── */
const RECURSION_CUTOFF: RecursionCutoff = DEFAULT_RECURSION_CUTOFF;
const BUCKET_SIZE: BucketSize = DEFAULT_BLOCKS_PER_BUCKET;
const POSITIONS_PER_BLOCK: BlockSize = DEFAULT_POSITIONS_PER_BLOCK;
const INITIAL_STASH_OVERFLOW_SIZE: StashSize = DEFAULT_STASH_OVERFLOW_SIZE; // default = 40
const STASH_GROWTH_INCREMENT: usize = 10;

const BLOCK_SIZE: BlockSize = 64;

/* ── Experiment parameters ────────────────────────────────────────── */
const DET_NUM_TESTS: usize = 1_000_000_000; // 10^9 deterministic accesses

/// We give the ORAM a large ceiling stash so it never hard-aborts during
/// the experiment; we observe the true occupancy distribution freely.
const EXPERIMENT_STASH_CEILING: StashSize = 500;

/// Histogram bucket count — tracks occupancies 0 … MAX_HIST_STASH.
/// Any occupancy ≥ MAX_HIST_STASH is clamped into the last bucket and
/// flagged in the log (should never occur with a 500-block ceiling).
const MAX_HIST_STASH: usize = 501;

/// How often to emit a progress line to stdout.
const PROGRESS_INTERVAL: usize = 10_000_000; // every 10 M ops

/* ── Log directory ────────────────────────────────────────────────── */
const LOG_DIR: &str = "./exp-results/results/exp-stash-failures";

// ──────────────────────────────────────────────────────────────────────────────
// Helper: delete directory if it exists
// ──────────────────────────────────────────────────────────────────────────────
fn delete_dir_if_exists(path_str: &str) -> std::io::Result<()> {
    let path = std::path::Path::new(path_str);
    if path.exists() && path.is_dir() {
        std::fs::remove_dir_all(path)?;
        println!("[setup] Removed old directory: {}", path_str);
    } else {
        println!("[setup] Directory not found (nothing to remove): {}", path_str);
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Core computation: minimum stash size for a given security parameter λ
//
// Given:
//   histogram[s] = number of batch accesses where post-eviction stash size == s
//   total_ops    = total number of batch accesses recorded
//   secparam     = λ  (we target failure probability ≤ 2^{-λ})
//
// We want the minimum S such that:
//   P(stash > S) = count(stash > S) / total_ops  ≤  2^{-λ}
//
// count(stash > S) = Σ_{s > S} histogram[s]  (suffix sum)
//
// Algorithm: scan from the largest bucket downward, accumulating the suffix
// sum. The first index s where the suffix count exceeds the threshold means
// we need S = s + 1.
// ──────────────────────────────────────────────────────────────────────────────
fn min_stash_for_secparam(histogram: &[u64], total_ops: u64, secparam: u64) -> usize {
    // Maximum number of overflow events we are willing to tolerate
    let max_allowed: f64 = (total_ops as f64) / 2f64.powi(secparam as i32);

    let mut suffix: u64 = 0; // running count(stash > s) as we walk right→left

    for s in (0..histogram.len()).rev() {
        // Invariant at this point: suffix == count(stash_size > s)
        if (suffix as f64) > max_allowed {
            // Stash size s is too small; minimum sufficient size is s + 1
            return s + 1;
        }
        suffix += histogram[s]; // extend suffix to cover stash_size == s
    }
    // Stash size 0 already satisfies the constraint (extremely unlikely but correct)
    0
}

/// Returns count(stash_size > stash_size_bound) — the number of accesses that
/// would overflow a stash of the given size.
fn overflow_count(histogram: &[u64], stash_size_bound: usize) -> u64 {
    let start = (stash_size_bound + 1).min(histogram.len());
    histogram[start..].iter().sum()
}

// ──────────────────────────────────────────────────────────────────────────────
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Database size(s) for the experiment are not specified.");
        eprintln!("E.g.: `./experiment 512 1024 2048`");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Please specify at least one database size as a command-line argument.",
        )));
    }

    let mut db_size_list: Vec<u64> = Vec::new();
    for arg in &args[1..] {
        db_size_list.push(arg.parse()?);
    }

    let mut rng = OsRng;

    // m = 1 is equivalent to standard (single-access) Path ORAM
    let batch_sizes: Vec<u64> = vec![1, 2, 4, 8, 16, 32];

    // Security parameters: we target P(failure) ≤ 2^{-λ} for each λ
    let secur_params: Vec<u64> = vec![16, 32];

    // ── (Re)create log directory ───────────────────────────────────────────
    let _ = delete_dir_if_exists(LOG_DIR);
    fs::create_dir_all(LOG_DIR)?;
    println!("[setup] Log directory ready: {}", LOG_DIR);

    // ── Summary log (one file for all (N, m) combos) ──────────────────────
    let summary_path = format!("{}/summary.log", LOG_DIR);
    let mut summary = fs::File::create(&summary_path)?;

    writeln!(summary, "╔══════════════════════════════════════════════════════════╗")?;
    writeln!(summary, "║          Stash-Overflow Experiment — Summary             ║")?;
    writeln!(summary, "╚══════════════════════════════════════════════════════════╝")?;
    writeln!(summary)?;
    writeln!(summary, "Deterministic accesses per config : {}", DET_NUM_TESTS)?;
    writeln!(summary, "Security parameters (λ)           : {:?}", secur_params)?;
    writeln!(summary, "Batch sizes (m)                   : {:?}", batch_sizes)?;
    writeln!(summary, "ORAM stash ceiling (experiment)   : {}", EXPERIMENT_STASH_CEILING)?;
    writeln!(summary, "Block size (bytes)                : {}", BLOCK_SIZE)?;
    writeln!(summary)?;
    writeln!(
        summary,
        "{:<8} {:<6} {:<14} {:<14} {:<14} {:<14}",
        "N", "m", "λ=16 stash", "λ=16 P(ovfl)", "λ=32 stash", "λ=32 P(ovfl)"
    )?;
    writeln!(summary, "{}", "─".repeat(76))?;

    // ── Main experiment loop ───────────────────────────────────────────────
    for db_size in &db_size_list {
        let num_leaves = db_size / 2;
        println!(
            "\n══════ Experiment: N (leaves) = {}  (db_size = {}) ══════",
            num_leaves, db_size
        );

        for batch_size in &batch_sizes {
            let t_start = Instant::now();
            println!("  ┌─ m = {} ──────────────────────────────────", batch_size);

            // ── Per-(N, m) log file ────────────────────────────────────────
            let log_path = format!("{}/N{}_m{}.log", LOG_DIR, num_leaves, batch_size);
            let mut log = fs::File::create(&log_path)?;

            writeln!(log, "╔══════════════════════════════════════════════════════════╗")?;
            writeln!(log, "║              Stash-Overflow Experiment Run               ║")?;
            writeln!(log, "╚══════════════════════════════════════════════════════════╝")?;
            writeln!(log)?;
            writeln!(log, "N (leaves)                : {}", num_leaves)?;
            writeln!(log, "db_size                   : {}", db_size)?;
            writeln!(log, "batch_size (m)            : {}", batch_size)?;
            writeln!(log, "block_size (bytes)        : {}", BLOCK_SIZE)?;
            writeln!(log, "deterministic_tests       : {}", DET_NUM_TESTS)?;
            writeln!(log, "experiment_stash_ceiling  : {}", EXPERIMENT_STASH_CEILING)?;
            writeln!(log, "initial_stash_overflow    : {}", INITIAL_STASH_OVERFLOW_SIZE)?;
            writeln!(log, "stash_growth_increment    : {}", STASH_GROWTH_INCREMENT)?;
            writeln!(log)?;

            // ── Initialise ORAM ────────────────────────────────────────────
            println!("  │  Initialising ORAM and writing database …");

            let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(*db_size as usize);
            for _ in 0..*db_size {
                let mut block = [0u8; BLOCK_SIZE];
                rng.fill_bytes(&mut block);
                database.push(block);
            }

            let mut batch_oram = PathOram::<
                BlockValue<BLOCK_SIZE>,
                BUCKET_SIZE,
                POSITIONS_PER_BLOCK,
            >::new_with_parameters(
                *db_size,
                &mut rng,
                EXPERIMENT_STASH_CEILING,
                RECURSION_CUTOFF,
                *batch_size as usize,
            )?;

            // Populate with initial writes (not counted toward the measurement)
            for (i, bytes) in database.iter().enumerate() {
                batch_oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
            }

            // ── Measurement phase ──────────────────────────────────────────
            println!("  │  Running {} deterministic accesses …", DET_NUM_TESTS);

            // histogram[s] = number of batch ops where post-eviction stash size == s
            let mut histogram: Vec<u64> = vec![0u64; MAX_HIST_STASH];
            let mut max_observed: usize = 0;
            let mut clamped_events: u64 = 0; // occupancy ≥ MAX_HIST_STASH (should be 0)

            for i in 0..DET_NUM_TESTS {
                // Build deterministic index batch: {offset, offset+1, …} mod db_size
                let offset = Address::try_from(i)?;
                let indices: Vec<Address> = (0..*batch_size)
                    .map(|n| (n + offset * (*batch_size)) % db_size)
                    .collect();

                let _ = batch_oram.read_with_batch(indices, &mut rng, true)?;

                // Record post-eviction stash occupancy
                let occ = batch_oram.stash.blocks.len();
                if occ < MAX_HIST_STASH {
                    histogram[occ] += 1;
                } else {
                    histogram[MAX_HIST_STASH - 1] += 1; // clamp
                    clamped_events += 1;
                }
                if occ > max_observed {
                    max_observed = occ;
                }

                if (i + 1) % PROGRESS_INTERVAL == 0 {
                    println!(
                        "  │  [{}/{}] ops done  |  max stash so far: {}",
                        i + 1,
                        DET_NUM_TESTS,
                        max_observed
                    );
                }
            }

            let total_ops = DET_NUM_TESTS as u64;
            let elapsed = t_start.elapsed();

            // ── Log histogram ──────────────────────────────────────────────
            writeln!(log, "══ Stash Occupancy Histogram ══════════════════════════════")?;
            writeln!(log, "Total batch accesses recorded : {}", total_ops)?;
            writeln!(log, "Max observed stash size       : {}", max_observed)?;
            if clamped_events > 0 {
                writeln!(
                    log,
                    "WARNING: {} events clamped at histogram ceiling ({}).",
                    clamped_events,
                    MAX_HIST_STASH - 1
                )?;
            }
            writeln!(log)?;
            writeln!(
                log,
                "{:<14} {:>18} {:>22} {:>22}",
                "StashSize(s)", "Count", "P(size=s)", "P(size>s)  [tail]"
            )?;
            writeln!(log, "{}", "─".repeat(78))?;

            // Precompute suffix sums for tail probabilities in the table
            let mut suffix_counts: Vec<u64> = vec![0u64; max_observed + 2];
            for s in (0..=max_observed).rev() {
                suffix_counts[s] = suffix_counts
                    .get(s + 1)
                    .copied()
                    .unwrap_or(0)
                    + histogram[s];
            }

            for s in 0..=max_observed {
                if histogram[s] == 0 {
                    continue;
                }
                let p_eq = histogram[s] as f64 / total_ops as f64;
                // P(stash > s) = suffix_counts[s] - histogram[s] = suffix_counts[s+1]
                let tail_count = suffix_counts.get(s + 1).copied().unwrap_or(0);
                let p_tail = tail_count as f64 / total_ops as f64;
                writeln!(
                    log,
                    "{:<14} {:>18} {:>22.6e} {:>22.6e}",
                    s, histogram[s], p_eq, p_tail
                )?;
            }
            writeln!(log)?;

            // ── Log required stash sizes per security parameter ────────────
            writeln!(
                log,
                "══ Required Stash Size per Security Parameter ═════════════"
            )?;
            writeln!(
                log,
                "   Goal: min S  s.t.  P(stash > S) ≤ 2^{{-λ}}"
            )?;
            writeln!(log)?;
            writeln!(
                log,
                "{:<8} {:>16} {:>22} {:>18} {:>18}",
                "λ", "RequiredStash(S)", "FailureThreshold", "OverflowCount", "ObservedP(ovfl)"
            )?;
            writeln!(log, "{}", "─".repeat(84))?;

            let mut summary_row_parts: Vec<String> = Vec::new();

            for &sp in &secur_params {
                let required_s = min_stash_for_secparam(&histogram, total_ops, sp);
                let threshold = (total_ops as f64) / 2f64.powi(sp as i32);
                // How many actual overflows occur if stash size == required_s?
                let ovfl_count = overflow_count(&histogram, required_s);
                let observed_p = ovfl_count as f64 / total_ops as f64;

                writeln!(
                    log,
                    "{:<8} {:>16} {:>22.6e} {:>18} {:>18.6e}",
                    sp, required_s, threshold, ovfl_count, observed_p
                )?;

                // Collect for summary line
                summary_row_parts.push(format!("{:<14}", required_s));
                summary_row_parts.push(format!("{:<14.3e}", observed_p));

                println!(
                    "  │  λ={:2}: required stash = {}  (observed P(ovfl) = {:.3e})",
                    sp, required_s, observed_p
                );
            }

            // ── Timing ────────────────────────────────────────────────────
            writeln!(log)?;
            writeln!(log, "══ Timing ══════════════════════════════════════════════════")?;
            writeln!(log, "Elapsed time: {:.2?}", elapsed)?;
            writeln!(
                log,
                "Throughput  : {:.2} Mops/s",
                (total_ops as f64) / elapsed.as_secs_f64() / 1e6
            )?;

            println!(
                "  └─ Done. Elapsed: {:.2?}  |  Log: {}",
                elapsed, log_path
            );

            // ── Write summary row ──────────────────────────────────────────
            // Columns: N, m, λ=16 stash, λ=16 P(ovfl), λ=32 stash, λ=32 P(ovfl)
            let sp_fields = if summary_row_parts.len() >= 4 {
                format!(
                    "{:<14} {:<14} {:<14} {:<14}",
                    summary_row_parts[0],
                    summary_row_parts[1],
                    summary_row_parts[2],
                    summary_row_parts[3]
                )
            } else {
                summary_row_parts.join("  ")
            };
            writeln!(
                summary,
                "{:<8} {:<6} {}",
                num_leaves, batch_size, sp_fields
            )?;
        }

        writeln!(summary)?;
    }

    writeln!(summary)?;
    writeln!(summary, "All experiments complete.")?;

    println!("\n✓ All experiments complete. Results written to: {}", LOG_DIR);
    println!("  Summary file: {}", summary_path);

    Ok(())
}