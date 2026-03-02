use oram::path_oram::{
    DEFAULT_BLOCKS_PER_BUCKET, DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF,
    DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{
    path_oram, Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff,
    StashSize,
};

use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::HashSet;

const RECURSION_CUTOFF: RecursionCutoff = DEFAULT_RECURSION_CUTOFF;
// const BUCKET_SIZE: BucketSize = 10;
const BUCKET_SIZE: BucketSize = DEFAULT_BLOCKS_PER_BUCKET;
const POSITIONS_PER_BLOCK: BlockSize = DEFAULT_POSITIONS_PER_BLOCK;
const INITIAL_STASH_OVERFLOW_SIZE: StashSize = DEFAULT_STASH_OVERFLOW_SIZE;

const BLOCK_SIZE: BlockSize = 64;
const DB_SIZE: Address = 512;
const NUM_BATCH_TESTS: usize = 1000;
const BATCH_SIZE: usize = 8;

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

    // Create a random byte database matching BlockValue's expected shape
    let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(DB_SIZE as usize);
    for _ in 0..(DB_SIZE as usize) {
        let mut block = [0u8; BLOCK_SIZE];
        rng.fill_bytes(&mut block);
        database.push(block);
    }

    // Initialize and populate (normal) path oram.
    let mut oram =
        PathOram::<BlockValue<BLOCK_SIZE>, BUCKET_SIZE, POSITIONS_PER_BLOCK>::new_with_parameters(
            DB_SIZE,
            &mut rng,
            INITIAL_STASH_OVERFLOW_SIZE,
            RECURSION_CUTOFF,
        )?;
    for (i, bytes) in database.iter().enumerate() {
        oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
    }

    // Initialize and populate batch path oram (initially using normal writes).
    let mut batch_oram =
        PathOram::<BlockValue<BLOCK_SIZE>, BUCKET_SIZE, POSITIONS_PER_BLOCK>::new_with_parameters(
            DB_SIZE,
            &mut rng,
            INITIAL_STASH_OVERFLOW_SIZE,
            RECURSION_CUTOFF,
        )?;
    for (i, bytes) in database.iter().enumerate() {
        batch_oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
    }

    println!("Initial ORAM population completed.\n\n");
    println!(
        "Starting experiment with the following parameters:\n
    N = {} blocks, Block Size = {} bytes, Batch Size = {}\n\n",
        DB_SIZE, BLOCK_SIZE, BATCH_SIZE
    );
    // Experiment: run monte-carlo
    for i in 0..NUM_BATCH_TESTS {
        // Get random indices with the size of batch.
        let indices = random_distinct_indices(&mut rng, BATCH_SIZE, DB_SIZE);

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
        println!("Round {} passed", i);
    }

    Ok(())
}
