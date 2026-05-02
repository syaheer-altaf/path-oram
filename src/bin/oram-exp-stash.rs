use oram::path_oram::{
    DEFAULT_BLOCKS_PER_BUCKET, DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF,
    DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{
    Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff, StashSize,
};

use rand::rngs::OsRng;
use rand::RngCore;

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
const RAND_NUM_TESTS: usize = 10000;                  // number of tests for random accesses to a batch of indices.
const DET_NUM_TESTS: usize = RAND_NUM_TESTS * 10;        // number of tests for deterministic (worst-case) accesses to a batch of indices.

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

fn random_indices(rng: &mut OsRng, count: usize, upper: Address) -> Vec<Address> {
    let mut indices = Vec::with_capacity(count);

    for _ in 0..count {
        indices.push(rng.next_u64() % upper);
    }

    indices
}

/*
Experimental Setup:
---------------------
1) Initialize ORAM of size db_size with random bytes.
2) Warm-up phase: make 10^9 batched accesses, (distinct) indices of batch size are requested uniformly at random
   with replacement from the range [0, N-1].
3) Measurement phase: make deterministic round-robin accesses, 10^9 rounds.
*/
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Collect all arguments into a Vector of Strings
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        println!("Database size(s) for the experiment are not specified.");
        println!("E.g.: `./experiment 512 1024 2048`");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Please specify the database size in the argument before running the experiment.",),
        )));
    }
    let mut rng = OsRng;
    let mut db_size_list: Vec<u64> = vec![];

    for i in 1..args.len() {
        db_size_list.push(args[i].parse()?);
    }
    // m = 1 is equivalent to path oram with a single access
    let batch_sizes: Vec<u64> = vec![1, 2, 4, 8, 16, 32];

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

            // Warm-up phase
            for _ in 0..RAND_NUM_TESTS {
                // Get random indices with the size of batch.
                let indices = random_indices(&mut rng, *batch_size as usize, db_size);

                // Random batched accesses to path oram
                let _: Vec<BlockValue<BLOCK_SIZE>> =
                    batch_oram.read_with_batch(indices, &mut rng, false)?;
            }
            // Measurement phase, {0,1,2,..,N,0,1,...} in batches.
            for i in 0..DET_NUM_TESTS {
                let mut indices: Vec<Address> = vec![];

                // Collect indices deterministically
                let offset: Address = Address::try_from(i)?;
                for n in 0..*batch_size {
                    let idx: Address = (n + offset * (*batch_size)) % db_size;
                    indices.push(idx);
                }

                let _: Vec<BlockValue<BLOCK_SIZE>> = batch_oram.read_with_batch(indices, &mut rng, true)?;
            }
        }
        println!("Experiment for N = {} has completed.", db_size / 2);
    }

    Ok(())
}
