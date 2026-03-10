use oram::path_oram::{
    DEFAULT_BLOCKS_PER_BUCKET, DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF,
    DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{
    Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff,
    StashSize,
};

use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::HashSet;

/*
 * NOTE: The convention used in this crate is slightly different from the original Path ORAM;
 * In particular, N (as some power of two) = number_of_leaves in a complete tree = DB_SIZE / 2.
*/
const RECURSION_CUTOFF: RecursionCutoff = DEFAULT_RECURSION_CUTOFF;
// const BUCKET_SIZE: BucketSize = 10;
const BUCKET_SIZE: BucketSize = DEFAULT_BLOCKS_PER_BUCKET;
const POSITIONS_PER_BLOCK: BlockSize = DEFAULT_POSITIONS_PER_BLOCK;
const INITIAL_STASH_OVERFLOW_SIZE: StashSize = DEFAULT_STASH_OVERFLOW_SIZE;

const BLOCK_SIZE: BlockSize = 64;
const NUM_TESTS: usize = 500;

fn delete_dir_if_exists(dir_path_str: &str) -> std::io::Result<()> {
    let path = std::path::Path::new(dir_path_str);

    if path.exists() && path.is_dir() {
        // Attempt to remove the directory and all its contents
        std::fs::remove_dir_all(path)?;
        println!("Directory removed: {}", dir_path_str);
    } else {
        println!("Directory not found or is a file: {}", dir_path_str);
    }

    Ok(())
}

fn random_distinct_indices(rng: &mut OsRng, count: usize, upper: Address) -> Vec<Address> {
    let mut seen = HashSet::with_capacity(count);
    let mut indices = Vec::with_capacity(count);

    while indices.len() < count {
        let candidate = rng.next_u64() % upper;
        if seen.insert(candidate) {
            indices.push(candidate);
        }
    }

    indices
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = OsRng;
    let db_size_list: Vec<u64> = vec![256, 512, 1024, 2048, 4096];
    // m = 1 is equivalent to path oram with a single access
    let batch_sizes: Vec<u64> = vec![1, 2];

    // delete old experiment results (if any)

    let _ = delete_dir_if_exists("./exp-results/results");

    for db_size in db_size_list {
        println!("Experiment for N = {} is in process..", db_size / 2);

        for batch_size in &batch_sizes {
            println!("\t* Working with m = {}..", &batch_size);
            // Create a random byte database matching BlockValue's expected shape
            let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(db_size as usize);
            for _ in 0..(db_size) {
                let mut block = [0u8; BLOCK_SIZE];
                rng.fill_bytes(&mut block);
                database.push(block);
            }

            // Initialize and populate (normal) path oram.
            let mut oram =
        PathOram::<BlockValue<BLOCK_SIZE>, BUCKET_SIZE, POSITIONS_PER_BLOCK>::new_with_parameters(
            db_size,
            &mut rng,
            INITIAL_STASH_OVERFLOW_SIZE,
            RECURSION_CUTOFF,
            1,
        )?;
            for (i, bytes) in database.iter().enumerate() {
                oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
            }

            // Initialize and populate batch path oram (initially using normal writes).
            let mut batch_oram = PathOram::<
                BlockValue<BLOCK_SIZE>,
                BUCKET_SIZE,
                POSITIONS_PER_BLOCK,
            >::new_with_parameters(
                db_size,
                &mut rng,
                INITIAL_STASH_OVERFLOW_SIZE,
                RECURSION_CUTOFF,
                *batch_size as usize,
            )?;
            for (i, bytes) in database.iter().enumerate() {
                batch_oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
            }

            // Experiment: run monte-carlo
            for _ in 0..NUM_TESTS {
                // Get random indices with the size of batch.
                let indices = random_distinct_indices(&mut rng, *batch_size as usize, db_size);

                // 1) make single, sequential accesses the size of batch
                let mut oram_reads: Vec<[u8; BLOCK_SIZE]> = vec![];

                for i in indices.iter().copied() {
                    let read = oram.read(i as Address, &mut rng, true)?;
                    oram_reads.push(read.data);
                }

                // 2) batched accesses to path oram
                let batch_oram_reads: Vec<[u8; BLOCK_SIZE]> = (batch_oram
                    .read_with_batch(indices, &mut rng, true)?)
                .iter()
                .map(|b| b.data)
                .collect();

                if oram_reads != batch_oram_reads {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("gg, mismatch!",),
                    )));
                }
            }
        }
        println!("Experiment for N = {} has completed.", db_size / 2);
    }

    Ok(())
}
