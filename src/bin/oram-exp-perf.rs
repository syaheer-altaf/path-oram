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

const BLOCK_SIZE: BlockSize = 64;
const DET_NUM_TESTS: usize = 1000; // monte carlo
/* ── Log directory ────────────────────────────────────────────────── */
const LOG_DIR: &str = "./exp-results/results/exp-perf";

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

// Make relative comparison and record the speedup using batched accesses in a log file
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
    let batch_sizes: Vec<u64> = vec![1, 2, 4, 8, 16, 32];

    // ── (Re)create log directory ───────────────────────────────────────────
    let _ = delete_dir_if_exists(LOG_DIR);
    fs::create_dir_all(LOG_DIR)?;
    println!("[setup] Log directory ready: {}", LOG_DIR);

    // ── Summary CSV (one row per (N, m)) ───────────────────────────────────
    let summary_path = format!("{}/summary.csv", LOG_DIR);
    let mut summary_file = fs::File::create(&summary_path)?;
    writeln!(
        summary_file,
        "n,m,trials,total_single_ns,total_batch_ns,mean_single_ns,mean_batch_ns,speedup"
    )?;

    // ── Main experiment loop ───────────────────────────────────────────────
    for db_size in &db_size_list {
        let db_size = *db_size;
        let n = db_size / 2; // logical database size shown in output
        println!("Experiment for N = {} is in process..", n);

        // Per-N detail CSV: one row per (m, trial)
        let detail_path = format!("{}/N{}.csv", LOG_DIR, n);
        let mut detail_file = fs::File::create(&detail_path)?;
        writeln!(detail_file, "m,trial,single_ns,batch_ns,speedup")?;

        for batch_size in &batch_sizes {
            let batch_size = *batch_size;
            println!("\t* Working with m = {}..", batch_size);

            // ── Build identical random database ───────────────────────────
            let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(db_size as usize);
            for _ in 0..db_size {
                let mut block = [0u8; BLOCK_SIZE];
                rng.fill_bytes(&mut block);
                database.push(block);
            }

            // ── Initialise single-access ORAM ──────────────────────────────
            let mut p_oram = PathOram::<
                BlockValue<BLOCK_SIZE>,
                BUCKET_SIZE,
                POSITIONS_PER_BLOCK,
            >::new_with_parameters(
                db_size,
                &mut rng,
                INITIAL_STASH_OVERFLOW_SIZE,
                RECURSION_CUTOFF,
                1, // m = 1  ≡  standard Path ORAM
            )?;
            for (i, bytes) in database.iter().enumerate() {
                p_oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
            }

            // ── Initialise batch ORAM ──────────────────────────────────────
            let mut batch_oram = PathOram::<
                BlockValue<BLOCK_SIZE>,
                BUCKET_SIZE,
                POSITIONS_PER_BLOCK,
            >::new_with_parameters(
                db_size,
                &mut rng,
                INITIAL_STASH_OVERFLOW_SIZE,
                RECURSION_CUTOFF,
                batch_size as usize,
            )?;
            for (i, bytes) in database.iter().enumerate() {
                batch_oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
            }

            // ── Timed trials ───────────────────────────────────────────────
            let mut total_single_ns: u128 = 0;
            let mut total_batch_ns: u128 = 0;

            for i in 0..DET_NUM_TESTS {
                // Deterministic index set shared by both ORAM variants
                let offset = Address::try_from(i)?;
                let indices: Vec<Address> = (0..batch_size)
                    .map(|n| (n + offset * batch_size) % db_size)
                    .collect();

                // ── Time m sequential single reads ─────────────────────────
                let t_single = Instant::now();
                for &idx in &indices {
                    let _: BlockValue<BLOCK_SIZE> = p_oram.read(idx, &mut rng, false)?;
                }
                let single_ns = t_single.elapsed().as_nanos();

                // ── Time one batched read of the same m indices ────────────
                let t_batch = Instant::now();
                let _: Vec<BlockValue<BLOCK_SIZE>> =
                    batch_oram.read_with_batch(indices, &mut rng, false)?;
                let batch_ns = t_batch.elapsed().as_nanos();

                total_single_ns += single_ns;
                total_batch_ns += batch_ns;

                // Guard against zero-duration batch (shouldn't happen, but avoids inf)
                let trial_speedup = if batch_ns > 0 {
                    single_ns as f64 / batch_ns as f64
                } else {
                    f64::NAN
                };

                writeln!(
                    detail_file,
                    "{},{},{},{},{:.6}",
                    batch_size, i, single_ns, batch_ns, trial_speedup
                )?;
            }

            let mean_single = total_single_ns / DET_NUM_TESTS as u128;
            let mean_batch  = total_batch_ns  / DET_NUM_TESTS as u128;
            let aggregate_speedup = if total_batch_ns > 0 {
                total_single_ns as f64 / total_batch_ns as f64
            } else {
                f64::NAN
            };

            println!(
                "\t  m={}: mean single = {} ns, mean batch = {} ns, speedup = {:.3}x",
                batch_size, mean_single, mean_batch, aggregate_speedup
            );

            writeln!(
                summary_file,
                "{},{},{},{},{},{},{},{:.6}",
                n,
                batch_size,
                DET_NUM_TESTS,
                total_single_ns,
                total_batch_ns,
                mean_single,
                mean_batch,
                aggregate_speedup
            )?;
        }

        println!("Experiment for N = {} has completed.", n);
    }

    println!("[done] Results written to {}", LOG_DIR);
    Ok(())
}