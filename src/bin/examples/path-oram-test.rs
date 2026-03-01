#[warn(unused_imports)]
use oram::path_oram::{
    DEFAULT_POSITIONS_PER_BLOCK, DEFAULT_RECURSION_CUTOFF,
    DEFAULT_STASH_OVERFLOW_SIZE,
};
use oram::{Address, BlockSize, BlockValue, BucketSize, Oram, PathOram, RecursionCutoff, StashSize};

use rand::rngs::OsRng;
use rand::{Rng, RngCore};
use rand::distributions::Uniform;

const RECURSION_CUTOFF: RecursionCutoff = DEFAULT_RECURSION_CUTOFF;
const BUCKET_SIZE: BucketSize = 3;
const POSITIONS_PER_BLOCK: BlockSize = DEFAULT_POSITIONS_PER_BLOCK;
const INITIAL_STASH_OVERFLOW_SIZE: StashSize = DEFAULT_STASH_OVERFLOW_SIZE;

const BLOCK_SIZE: BlockSize = 64;
const DB_SIZE: Address = 8192;

fn qsort(arr: &mut [u8]) {
    if arr.len() <= 1 {
        return;
    }
    let last_idx = arr.len() - 1;
    let pivot = arr[last_idx];

    let mut i = 0;
    for j in 0..last_idx {
        if arr[j] <= pivot {
            arr.swap(i, j);
            i += 1;
        }
    }

    arr.swap(i, last_idx); // i is the pivot index
    qsort(&mut arr[0..i]);
    qsort(&mut arr[(i + 1)..]);
}

fn is_sort(arr: &[u8]) -> bool {
    let mut checker: bool = true;
    for i in 1..arr.len() {
        if arr[i - 1] > arr[i] {
            checker = false;
            break;
        }
    }
    return checker;
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One RNG for everything (CryptoRng + RngCore, compatible with oram)
    let mut rng = OsRng;

    // Create a random byte database matching BlockValue's expected shape
    let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(DB_SIZE as usize);
    for _ in 0..(DB_SIZE as usize) {
        let mut block = [0u8; BLOCK_SIZE];
        rng.fill_bytes(&mut block);
        database.push(block);
    }

    // println!("database[0] (normal access):\n\n {:?}\n", database[0]);
    // println!("is database[0] sorted (normal access)? {}", is_sort(&database[0]));

    // Initialize oram
    let mut oram =
        PathOram::<BlockValue<BLOCK_SIZE>, BUCKET_SIZE, POSITIONS_PER_BLOCK>::new_with_parameters(
            DB_SIZE,
            &mut rng,
            INITIAL_STASH_OVERFLOW_SIZE,
            RECURSION_CUTOFF,
        )?;

    // Populate oram
    for (i, bytes) in database.iter().enumerate() {
        oram.write(i as Address, BlockValue::new(*bytes), &mut rng, false)?;
    }

    // Modify the database via oram
    // let mut arr = oram.read(0 as Address, &mut rng, true).unwrap().data;
    // qsort(&mut arr);
    // oram.write(0 as Address, BlockValue::new(arr), &mut rng, true)?;
    // println!("database[0] (oram access read):\n\n {:?}\n", oram.read(0 as Address, &mut rng, false).unwrap().data);
    // println!("is database[0] sorted (oram access read)? {}", is_sort(&oram.read(0 as Address, &mut rng, false).unwrap().data));

    // Run random accesses
    for _ in 0..100 {
        let idx = rng.sample(Uniform::new(0u32, 1024u32));
        let _ = oram.read(idx as Address, &mut rng, true);
    }
    Ok(())
}