// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is dual-licensed under either the MIT license found in the
// LICENSE-MIT file in the root directory of this source tree or the Apache
// License, Version 2.0 found in the LICENSE-APACHE file in the root directory
// of this source tree. You may select, at your option, one of the above-listed licenses.

//! A trait representing a Path ORAM stash.

use crate::{
    bucket::{Bucket, PathOramBlock},
    utils::{bitonic_sort_by_keys, CompleteBinaryTreeIndex, TreeIndex, create_path_if_not_exists, append_to_file},
    Address, BucketSize, OramBlock, OramError, StashSize,
};

use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};

const STASH_GROWTH_INCREMENT: usize = 10;

#[derive(Debug)]
/// A fixed-size, obliviously accessed Path ORAM stash data structure implemented using oblivious sorting.
pub struct ObliviousStash<V: OramBlock> {
    blocks: Vec<PathOramBlock<V>>,
    path_size: StashSize,
    m_batch: usize, // If single accesses are to be expected, let m_batch = 1.
}

impl<V: OramBlock> ObliviousStash<V> {
    fn len(&self) -> usize {
        self.blocks.len()
    }
}

impl<V: OramBlock> ObliviousStash<V> {
    pub fn new(path_size: StashSize, overflow_size: StashSize, m_batch: usize) -> Result<Self, OramError> {
        let num_stash_blocks: usize = (path_size + overflow_size).try_into()?;

        Ok(Self {
            blocks: vec![PathOramBlock::<V>::dummy(); num_stash_blocks],
            path_size,
            m_batch,
        })
    }

    pub fn write_to_path<const Z: BucketSize>(
        &mut self,
        physical_memory: &mut [Bucket<V, Z>],
        position: TreeIndex,
        is_log: bool,
    ) -> Result<(), OramError> {
        let height = position.ct_depth();
        let mut level_assignments = vec![TreeIndex::MAX; self.len()];
        let mut level_counts = vec![0; usize::try_from(height)? + 1];

        // Assign all non-dummy blocks in the stash to either the path or the overflow.
        for (i, block) in self.blocks.iter().enumerate() {
            // If `block` is a dummy, the rest of this loop iteration will be a no-op, and the values don't matter.
            let block_is_dummy = block.ct_is_dummy();

            // Set up valid but meaningless input to the computation in case `block` is a dummy.
            let an_arbitrary_leaf: TreeIndex = 1 << height;
            let block_position =
                TreeIndex::conditional_select(&block.position, &an_arbitrary_leaf, block_is_dummy);

            // Assign the block to a bucket or to the overflow.
            let mut assigned = Choice::from(0);
            // Obliviously scan through the buckets from leaf to root,
            // assigning the block to the first empty bucket satisfying the invariant.
            for (level, count) in level_counts.iter_mut().enumerate().rev() {
                let level_bucket_full: Choice = count.ct_eq(&(u64::try_from(Z)?));

                let level_u64 = u64::try_from(level)?;
                let level_satisfies_invariant = block_position
                    .ct_node_on_path(level_u64, height)
                    .ct_eq(&position.ct_node_on_path(level_u64, height));

                let should_assign = level_satisfies_invariant
                    & (!level_bucket_full)
                    & (!block_is_dummy)
                    & (!assigned);
                assigned |= should_assign;

                let level_count_incremented = *count + 1;
                count.conditional_assign(&level_count_incremented, should_assign);
                level_assignments[i].conditional_assign(&level_u64, should_assign);
            }
            // If the block was not able to be assigned to any bucket, assign it to the overflow.
            level_assignments[i]
                .conditional_assign(&(TreeIndex::MAX - 1), (!assigned) & (!block_is_dummy));
        }

        // Assign dummy blocks to the remaining non-full buckets until all buckets are full.
        let mut exists_unfilled_levels: Choice = 1.into();
        let mut first_unassigned_block_index: usize = 0;
        // Unless the stash overflows, this loop will execute exactly once, and the inner `if` will not execute.
        // If the stash overflows, this loop will execute twice and the inner `if` will execute.
        // This difference in control flow will leak the fact that the stash has overflowed.
        // This is a violation of obliviousness, but the alternative is simply to fail.
        // If the stash is set large enough when the ORAM is initialized,
        // stash overflow will occur only with negligible probability.
        while exists_unfilled_levels.into() {
            // Make a pass over the stash, assigning dummy blocks to unfilled levels in the path.
            for (i, block) in self
                .blocks
                .iter()
                .enumerate()
                .skip(first_unassigned_block_index)
            {
                // Skip the last block. It is reserved for handling writes to uninitialized addresses.
                if i == self.blocks.len() - 1 {
                    break;
                }

                let block_free = block.ct_is_dummy();

                let mut assigned: Choice = 0.into();
                for (level, count) in level_counts.iter_mut().enumerate() {
                    let full = count.ct_eq(&(u64::try_from(Z)?));
                    let no_op = assigned | full | !block_free;

                    level_assignments[i].conditional_assign(&(u64::try_from(level))?, !no_op);
                    count.conditional_assign(&(*count + 1), !no_op);
                    assigned |= !no_op;
                }
            }

            // Check that all levels have been filled.
            exists_unfilled_levels = 0.into();
            for count in level_counts.iter() {
                let full = count.ct_eq(&(u64::try_from(Z)?));
                exists_unfilled_levels |= !full;
            }

            // If not, there must not have been enough dummy blocks remaining in the stash.
            // That is, the stash has overflowed.
            // So, extend the stash with STASH_GROWTH_INCREMENT more dummy blocks,
            // and repeat the process of trying to fill all unfilled levels with dummy blocks.
            if exists_unfilled_levels.into() {
                first_unassigned_block_index = self.blocks.len() - 1;

                self.blocks.resize(
                    self.blocks.len() + STASH_GROWTH_INCREMENT,
                    PathOramBlock::<V>::dummy(),
                );
                level_assignments.resize(
                    level_assignments.len() + STASH_GROWTH_INCREMENT,
                    TreeIndex::MAX,
                );

                log::warn!(
                    "Stash overflow occurred. Stash resized to {} blocks.",
                    self.blocks.len()
                );
            }
        }

        bitonic_sort_by_keys(&mut self.blocks, &mut level_assignments);

        // Write the first Z * height blocks into slots in the tree
        for depth in 0..=height {
            let bucket_to_write =
                &mut physical_memory[usize::try_from(position.ct_node_on_path(depth, height))?];
            for slot_number in 0..Z {
                let stash_index = (usize::try_from(depth)?) * Z + slot_number;

                bucket_to_write.blocks[slot_number] = self.blocks[stash_index];
            }
        }

        if is_log {
            // Bandwidth is fixed and easy to compute for this case;
            // do not log stash for single accesses for now.
            // let _ = append_to_file("./exp-results/bandwidth_single.log", self.path_size.to_string().as_str());
            // let _ = append_to_file("./exp-results/stash_single.log", self.occupancy().to_string().as_str());
        }
        Ok(())
    }

    pub fn access<F: Fn(&V) -> V>(
        &mut self,
        address: Address,
        new_position: TreeIndex,
        value_callback: F,
    ) -> Result<V, OramError> {
        let mut result: V = V::default();
        let mut found: Choice = 0.into();

        // Iterate over stash, updating the block with address `address` if one exists.
        for block in &mut self.blocks {
            let is_requested_index = block.address.ct_eq(&address);
            found.conditional_assign(&1.into(), is_requested_index);

            // Read current value of target block into `result`.
            result.conditional_assign(&block.value, is_requested_index);
            // Write new position into target block.
            block
                .position
                .conditional_assign(&new_position, is_requested_index);
            // If a write, write new value into target block.
            let value_to_write = value_callback(&result);
            block
                .value
                .conditional_assign(&value_to_write, is_requested_index);
        }

        // If a block with address `address` is not found,
        // initialize one by writing to the last block in the stash,
        // which will always be a dummy block.
        let last_block_index = self.blocks.len() - 1;
        let last_block = &mut self.blocks[last_block_index];
        assert!(bool::from(last_block.ct_is_dummy()));
        last_block.conditional_assign(
            &PathOramBlock {
                value: value_callback(&result),
                address,
                position: new_position,
            },
            !found,
        );

        // Return the value of the found block (or the default value, if no block was found)
        Ok(result)
    }

    pub fn read_from_path<const Z: crate::BucketSize>(
        &mut self,
        physical_memory: &mut [Bucket<V, Z>],
        position: TreeIndex,
        is_log: bool,
    ) -> Result<(), OramError> {
        let height = position.ct_depth();

        for i in (0..(self.path_size / u64::try_from(Z)?)).rev() {
            let bucket_index = position.ct_node_on_path(i, height);
            let bucket = physical_memory[usize::try_from(bucket_index)?];
            for slot_index in 0..Z {
                self.blocks[Z * (usize::try_from(i)?) + slot_index] = bucket.blocks[slot_index];
            }
        }

        if is_log {
            // Bandwidth is fixed and easy to compute for this case;
            // do not log stash for single accesses for now.
            // println!("\n\nread bandwidth:{}\n", self.path_size);
            // let _ = append_to_file("./exp-results/bandwidth_single.log", self.path_size.to_string().as_str());
            // let _ = append_to_file("./exp-results/stash_single.log", self.occupancy().to_string().as_str());
        }
        Ok(())
    }

    /*
        TODO: REVISE `read_from_path_union`, `write_to_path_union`, `batched_access`
     */
    // The following methods are used alongside `batched_access`.
    pub fn read_from_path_union<const Z: crate::BucketSize>(
        &mut self,
        physical_memory: &[Bucket<V, Z>],
        positions: &[TreeIndex],
        is_log: bool,
    ) -> Result<Vec<u64>, OramError> {
        use std::collections::HashSet;

        let mut paths_union: Vec<u64> = Vec::new();
        let mut seen: HashSet<u64> = HashSet::new();

        let fixed_path_block_count: usize = usize::try_from(self.path_size)?;
        let buckets_per_path = self.path_size / u64::try_from(Z)?;

        for &position in positions {
            let height = position.ct_depth();

            for i in (0..buckets_per_path).rev() {
                let bucket_index = position.ct_node_on_path(i, height);
                if seen.insert(bucket_index) {
                    paths_union.push(bucket_index);
                }
            }
        }

        let union_block_count = paths_union.len() * Z;

        // If the union needs a larger scratch prefix than the fixed single-path prefix,
        // preserve the existing overflow stash blocks by shifting them right.
        if union_block_count > fixed_path_block_count {
            let extra = union_block_count - fixed_path_block_count;
            let old_len = self.blocks.len();

            self.blocks
                .resize(old_len + extra, PathOramBlock::<V>::dummy());

            // Shift the old overflow region [fixed_path_block_count, old_len) to the right by `extra`.
            for i in (fixed_path_block_count..old_len).rev() {
                self.blocks[i + extra] = self.blocks[i];
            }

            // Optional but clean: blank out the newly opened region.
            for i in fixed_path_block_count..(fixed_path_block_count + extra) {
                self.blocks[i] = PathOramBlock::<V>::dummy();
            }
        }

        // Load the union buckets into the front scratch region.
        for (i, &bid) in paths_union.iter().enumerate() {
            let bucket = physical_memory[usize::try_from(bid)?];
            for slot_index in 0..Z {
                self.blocks[Z * i + slot_index] = bucket.blocks[slot_index];
            }
        }

        if is_log {
            let height = positions[0].ct_depth(); // Use the first position to get the height of the tree
            let n_size = 2_usize.pow(u32::try_from(height)?);
            let log_path_name = format!("./exp-results/results/N_{}/{}", n_size, self.m_batch);
            let bandwidth_log_filename = format!("{}/{}", log_path_name, "bandwidth_batch.log");
            let stash_log_filename = format!("{}/{}", log_path_name, "stash_batch.log");
            let _ = create_path_if_not_exists(&log_path_name);
            let _ = append_to_file(&bandwidth_log_filename, union_block_count.to_string().as_str());
            let _ = append_to_file(&stash_log_filename, self.stash_occupancy().to_string().as_str());
        }

        Ok(paths_union)
    }

    pub fn write_to_path_union<const Z: BucketSize>(
        &mut self,
        physical_memory: &mut [Bucket<V, Z>],
        union_buckets: &[TreeIndex],
        is_log: bool,
    ) -> Result<(), OramError> {
        use std::collections::HashSet;
        use subtle::Choice;

        if union_buckets.is_empty() {
            if is_log {
                let height = union_buckets[0].ct_depth(); // Use the first position to get the height of the tree
                let n_size = 2_usize.pow(u32::try_from(height)?);
                let log_path_name = format!("./exp-results/results/N_{}/{}", n_size, self.m_batch);
                let bandwidth_log_filename = format!("{}/{}", log_path_name, "bandwidth_batch.log");
                let stash_log_filename = format!("{}/{}", log_path_name, "stash_batch.log");
                let _ = create_path_if_not_exists(&log_path_name);
                let _ = append_to_file(&bandwidth_log_filename, "0");
                let _ = append_to_file(&stash_log_filename, self.stash_occupancy().to_string().as_str());
            }
            return Ok(());
        }

        // Deduplicate defensively, then order buckets deepest-to-shallowest so that
        // real blocks are evicted as deep as possible, analogous to write_to_path().
        let mut seen = HashSet::new();
        let mut ordered_union_buckets: Vec<TreeIndex> = union_buckets
            .iter()
            .map(|&b| b)
            .filter(|b| seen.insert(*b))
            .collect();

        ordered_union_buckets.sort_by(|a, b| {
            let da = a.ct_depth();
            let db = b.ct_depth();
            db.cmp(&da).then_with(|| a.cmp(b))
        });

        // For each stash block, record which union-bucket slot-group it is assigned to.
        // Values 0..ordered_union_buckets.len()-1 mean "assigned to that bucket".
        // TreeIndex::MAX - 1 means overflow.
        // TreeIndex::MAX means still unassigned (used mainly for dummies before fill).
        let mut bucket_assignments = vec![TreeIndex::MAX; self.blocks.len()];
        let mut bucket_counts = vec![0u64; ordered_union_buckets.len()];

        // We need a valid leaf for dummy blocks, though it will never matter because
        // assignments for dummies are gated by `!block_is_dummy`.
        let max_depth = ordered_union_buckets
            .iter()
            .map(|b| b.ct_depth())
            .max()
            .unwrap_or(0);
        let an_arbitrary_leaf: TreeIndex = 1u64 << max_depth;

        // Assign all non-dummy blocks either to some bucket in the union or to overflow.
        for (i, block) in self.blocks.iter().enumerate() {
            let block_is_dummy = block.ct_is_dummy();
            let block_position =
                TreeIndex::conditional_select(&block.position, &an_arbitrary_leaf, block_is_dummy);

            let mut assigned: Choice = 0.into();

            // Scan candidate union buckets from deepest to shallowest.
            for (bucket_slot, count) in bucket_counts.iter_mut().enumerate() {
                let bucket_index = ordered_union_buckets[bucket_slot];
                let bucket_depth = bucket_index.ct_depth();

                let bucket_full: Choice = count.ct_eq(&(u64::try_from(Z)?));

                // A block can be placed in bucket_index iff bucket_index lies on the
                // path from the root to block_position.
                let bucket_on_block_path = block_position
                    .ct_node_on_path(bucket_depth, block_position.ct_depth())
                    .ct_eq(&bucket_index);

                let should_assign =
                    bucket_on_block_path & (!bucket_full) & (!block_is_dummy) & (!assigned);
                assigned |= should_assign;

                let incremented = *count + 1;
                count.conditional_assign(&incremented, should_assign);

                bucket_assignments[i]
                    .conditional_assign(&(u64::try_from(bucket_slot)?), should_assign);
            }

            // If this real block could not be placed into any bucket in the union,
            // it remains in stash overflow.
            bucket_assignments[i]
                .conditional_assign(&(TreeIndex::MAX - 1), (!assigned) & (!block_is_dummy));
        }

        // Fill all remaining non-full buckets in the union with dummy blocks.
        let mut exists_unfilled_buckets: Choice = 1.into();
        let mut first_unassigned_block_index: usize = 0;

        while exists_unfilled_buckets.into() {
            for (i, block) in self
                .blocks
                .iter()
                .enumerate()
                .skip(first_unassigned_block_index)
            {
                // Preserve the library's convention: last block reserved for writes
                // to uninitialized addresses.
                if i == self.blocks.len() - 1 {
                    break;
                }

                let block_free = block.ct_is_dummy();
                let mut assigned: Choice = 0.into();

                for (bucket_slot, count) in bucket_counts.iter_mut().enumerate() {
                    let full = count.ct_eq(&(u64::try_from(Z)?));
                    let no_op = assigned | full | !block_free;

                    bucket_assignments[i]
                        .conditional_assign(&(u64::try_from(bucket_slot)?), !no_op);
                    count.conditional_assign(&(*count + 1), !no_op);
                    assigned |= !no_op;
                }
            }

            exists_unfilled_buckets = 0.into();
            for count in bucket_counts.iter() {
                let full = count.ct_eq(&(u64::try_from(Z)?));
                exists_unfilled_buckets |= !full;
            }

            // If not all buckets are filled, stash overflowed with respect to this union.
            if exists_unfilled_buckets.into() {
                first_unassigned_block_index = self.blocks.len() - 1;

                self.blocks.resize(
                    self.blocks.len() + STASH_GROWTH_INCREMENT,
                    PathOramBlock::<V>::dummy(),
                );
                bucket_assignments.resize(
                    bucket_assignments.len() + STASH_GROWTH_INCREMENT,
                    TreeIndex::MAX,
                );

                log::warn!(
                    "Stash overflow occurred during union writeback. Stash resized to {} blocks.",
                    self.blocks.len()
                );
            }
        }

        bitonic_sort_by_keys(&mut self.blocks, &mut bucket_assignments);

        // Write the first |union_buckets| * Z blocks back into the selected buckets.
        let mut write_bandwidth = 0usize;
        for (bucket_slot, &bucket_index) in ordered_union_buckets.iter().enumerate() {
            let bucket_to_write = &mut physical_memory[usize::try_from(bucket_index)?];
            for slot_number in 0..Z {
                let stash_index = bucket_slot * Z + slot_number;
                bucket_to_write.blocks[slot_number] = self.blocks[stash_index];
            }
            write_bandwidth += bucket_to_write.blocks.len();
        }

        // IMPORTANT:
        // Unlike the fixed-size single-path case, union writeback may use a variable-size
        // prefix of the stash. If we leave those blocks in-place, later accesses may not
        // overwrite all of them, leaving stale duplicates in stash.
        let written_block_count = ordered_union_buckets.len() * Z;
        for i in 0..written_block_count {
            self.blocks[i] = PathOramBlock::<V>::dummy();
        }

        if is_log {
            let height = union_buckets[0].ct_depth(); // Use the first position to get the height of the tree
            let n_size = 2_usize.pow(u32::try_from(height)?);
            let log_path_name = format!("./exp-results/results/N_{}/{}", n_size, self.m_batch);
            let bandwidth_log_filename = format!("{}/{}", log_path_name, "bandwidth_batch.log");
            let stash_log_filename = format!("{}/{}", log_path_name, "stash_batch.log");
            let _ = create_path_if_not_exists(&log_path_name);
            let _ = append_to_file(&bandwidth_log_filename, write_bandwidth.to_string().as_str());
            let _ = append_to_file(&stash_log_filename, self.stash_occupancy().to_string().as_str());
        }

        Ok(())
    }

    pub fn batched_access<F: Fn(Vec<&V>) -> Vec<V>>(
        &mut self,
        addresses: Vec<Address>,
        new_positions: Vec<TreeIndex>,
        value_callback: F,
    ) -> Result<Vec<V>, OramError> {
        if addresses.len() != new_positions.len() {
            return Err(OramError::InvalidConfigurationError {
                parameter_name: "batched_access input lengths".to_string(),
                parameter_value: format!(
                    "addresses has length {}, but new_positions has length {}",
                    addresses.len(),
                    new_positions.len()
                ),
            });
        }

        // For sanity and to avoid ambiguous semantics / duplicate stash entries,
        // require distinct logical addresses in one batch.
        // for i in 0..addresses.len() {
        //     for j in (i + 1)..addresses.len() {
        //         if addresses[i] == addresses[j] {
        //             return Err(OramError::InvalidConfigurationError {
        //                 parameter_name: "batched_access addresses".to_string(),
        //                 parameter_value: format!(
        //                     "duplicate logical address {} appears multiple times in one batch",
        //                     addresses[i]
        //                 ),
        //             });
        //         }
        //     }
        // }

        let mut results: Vec<V> = vec![V::default(); addresses.len()];
        let mut found: Vec<Choice> = vec![0.into(); addresses.len()];

        // First pass: read old values out of the stash.
        for block in &self.blocks {
            for i in 0..addresses.len() {
                let is_requested_index = block.address.ct_eq(&addresses[i]);

                found[i].conditional_assign(&1.into(), is_requested_index);
                results[i].conditional_assign(&block.value, is_requested_index);
            }
        }

        // Apply the batched callback once.
        let callback_input: Vec<&V> = results.iter().collect();
        let values_to_write = value_callback(callback_input);

        if values_to_write.len() != addresses.len() {
            return Err(OramError::InvalidConfigurationError {
                parameter_name: "batched_access callback output length".to_string(),
                parameter_value: format!(
                    "expected {}, got {}",
                    addresses.len(),
                    values_to_write.len()
                ),
            });
        }

        // Second pass: update all existing matching blocks in-place.
        for block in &mut self.blocks {
            for i in 0..addresses.len() {
                let is_requested_index = block.address.ct_eq(&addresses[i]);

                block
                    .position
                    .conditional_assign(&new_positions[i], is_requested_index);
                block
                    .value
                    .conditional_assign(&values_to_write[i], is_requested_index);
            }
        }

        // Count how many requested addresses were not found.
        let missing_count = found.iter().filter(|c| !bool::from(**c)).count();

        // Count currently available dummy slots.
        let available_dummy_count = self
            .blocks
            .iter()
            .filter(|block| bool::from(block.ct_is_dummy()))
            .count();

        // If needed, extend stash with dummy blocks so we can initialize all misses.
        if missing_count > available_dummy_count {
            self.blocks.resize(
                self.blocks.len() + (missing_count - available_dummy_count),
                PathOramBlock::<V>::dummy(),
            );
        }

        // Initialize one new stash block for each missing address.
        // We fill from the end, preferring dummy blocks near the back.
        let mut next_free_slot = self.blocks.len();

        for i in 0..addresses.len() {
            if !bool::from(found[i]) {
                loop {
                    if next_free_slot == 0 {
                        return Err(OramError::InvalidConfigurationError {
                            parameter_name: "stash dummy capacity".to_string(),
                            parameter_value: "no dummy block available for batch insertion"
                                .to_string(),
                        });
                    }

                    next_free_slot -= 1;

                    if bool::from(self.blocks[next_free_slot].ct_is_dummy()) {
                        break;
                    }
                }

                self.blocks[next_free_slot] = PathOramBlock {
                    value: values_to_write[i],
                    address: addresses[i],
                    position: new_positions[i],
                };
            }
        }

        Ok(results)
    }

    // Utilities for stash
    // pub fn occupancy(&self) -> StashSize {
    //     let mut result = 0;
    //     for i in self.path_size.try_into().unwrap()..(self.blocks.len()) {
    //         if !self.blocks[i].is_dummy() {
    //             result += 1;
    //         }
    //     }
    //     result
    // }

    pub fn stash_occupancy(&self) -> StashSize {
        let mut result = 0;
        for i in 0..(self.blocks.len()) {
            if !self.blocks[i].is_dummy() {
                result += 1;
            }
        }
        result
    }
}
