use oram::{linear_time_oram::LinearTimeOram, Address, BlockSize, BlockValue, Oram};

use rand::distributions::Uniform;
use rand::rngs::OsRng;
use rand::{Rng, RngCore};

const BLOCK_SIZE: BlockSize = 64;
const DB_SIZE: Address = 64;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = OsRng;

    // Create a random byte database.
    let mut database: Vec<[u8; BLOCK_SIZE]> = Vec::with_capacity(DB_SIZE as usize);
    for _ in 0..(DB_SIZE as usize) {
        let mut block = [0u8; BLOCK_SIZE];
        rng.fill_bytes(&mut block);
        database.push(block);
    }

    // Initialize linear-time oram.
    let mut oram = LinearTimeOram::<BlockValue<BLOCK_SIZE>>::new(DB_SIZE)?;

    // Populate oram (try batched-access--no difference than single access performance-wise).
    let indices: Vec<Address> = (0..DB_SIZE).collect();
    oram.write_with_batch(indices.clone(), database.iter().copied().map(BlockValue::new).collect(), &mut rng, false)?;

    // // Let's sneak incorrectness at a single block
    // let rand_idx = rng.sample(Uniform::new(0 as usize, DB_SIZE as usize));
    // let mut new_block = [0u8; BLOCK_SIZE];
    // rng.fill_bytes(&mut new_block);
    // oram.write(rand_idx as Address, BlockValue::new(new_block), &mut rng, false)?;

    let batch_read = oram.read_with_batch(indices.clone(), &mut rng, false);

    println!("Is correct for all instances?");
    let mut is_correct = true;
    for (i, b) in batch_read.unwrap().into_iter().enumerate() {
        if b.data != database[i] {
            println!("--not good @ index {}", i);
            is_correct = false;
            break;
        }
    }
    if is_correct {
        println!("--nice, all is good");
    } else {
        println!("--gg!");
    }
    Ok(())
}