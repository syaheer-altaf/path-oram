#[warn(unused_imports)]
use oram::path_oram::{
    DEFAULT_BLOCKS_PER_BUCKET, DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF, DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{
    Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff, StashSize,
};

use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::HashSet;

const RECURSION_CUTOFF: RecursionCutoff = DEFAULT_RECURSION_CUTOFF;
const BUCKET_SIZE: BucketSize = DEFAULT_BLOCKS_PER_BUCKET;
const POSITIONS_PER_BLOCK: BlockSize = DEFAULT_POSITIONS_PER_BLOCK;
const INITIAL_STASH_OVERFLOW_SIZE: StashSize = DEFAULT_STASH_OVERFLOW_SIZE;

const BLOCK_SIZE: BlockSize = 64;
const DB_SIZE: Address = 1024;
const NUM_BATCH_TESTS: usize = 100;
const BATCH_SIZE: usize = 64;

fn random_block(rng: &mut OsRng) -> [u8; BLOCK_SIZE] {
    let mut block = [0u8; BLOCK_SIZE];
    rng.fill_bytes(&mut block);
    block
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

    // Reference database.
    let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(DB_SIZE as usize);
    for _ in 0..(DB_SIZE as usize) {
        database.push(random_block(&mut rng));
    }

    // Initialize Path ORAM.
    let mut oram =
        PathOram::<BlockValue<BLOCK_SIZE>, BUCKET_SIZE, POSITIONS_PER_BLOCK>::new_with_parameters(
            DB_SIZE,
            &mut rng,
            INITIAL_STASH_OVERFLOW_SIZE,
            RECURSION_CUTOFF,
        )?;

    // Populate ORAM using ordinary writes.
    for (i, bytes) in database.iter().enumerate() {
        oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
    }

    println!("Initial ORAM population completed.");

    for round in 0..NUM_BATCH_TESTS {
        let indices = random_distinct_indices(&mut rng, BATCH_SIZE, DB_SIZE);

        // ------------------------------------------------------------
        // 1) Check batched read against reference database.
        // ------------------------------------------------------------
        let read_result = oram.read_with_batch(indices.clone(), &mut rng, false)?;
        for (j, &addr) in indices.iter().enumerate() {
            let expected = BlockValue::new(database[addr as usize]);
            assert_eq!(
                read_result[j], expected,
                "Round {}: read_with_batch mismatch at logical address {}",
                round, addr
            );
        }

        // ------------------------------------------------------------
        // 2) Prepare new random values for batched write.
        // ------------------------------------------------------------
        let mut new_raw_blocks: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(BATCH_SIZE);
        for _ in 0..BATCH_SIZE {
            new_raw_blocks.push(random_block(&mut rng));
        }

        let new_values: Vec<BlockValue<BLOCK_SIZE>> = new_raw_blocks
            .iter()
            .copied()
            .map(BlockValue::new)
            .collect();

        // ------------------------------------------------------------
        // 3) Check batched write returns old values.
        // ------------------------------------------------------------
        let old_values =
            oram.write_with_batch(indices.clone(), new_values, &mut rng, false)?;

        for (j, &addr) in indices.iter().enumerate() {
            let expected_old = BlockValue::new(database[addr as usize]);
            assert_eq!(
                old_values[j], expected_old,
                "Round {}: write_with_batch returned wrong old value at logical address {}",
                round, addr
            );
        }

        // Update the reference database.
        for (j, &addr) in indices.iter().enumerate() {
            database[addr as usize] = new_raw_blocks[j];
        }

        // ------------------------------------------------------------
        // 4) Check batched read after write sees the new values.
        // ------------------------------------------------------------
        let read_after_write = oram.read_with_batch(indices.clone(), &mut rng, false)?;
        for (j, &addr) in indices.iter().enumerate() {
            let expected_new = BlockValue::new(database[addr as usize]);
            assert_eq!(
                read_after_write[j], expected_new,
                "Round {}: post-write read_with_batch mismatch at logical address {}",
                round, addr
            );
        }

        if round % 10 == 0 {
            println!("Batch test round {} passed.", round);
        }
    }

    println!(
        "All {} batched read/write correctness checks passed.",
        NUM_BATCH_TESTS
    );

    Ok(())
}