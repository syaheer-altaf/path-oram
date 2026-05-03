// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is dual-licensed under either the MIT license found in the
// LICENSE-MIT file in the root directory of this source tree or the Apache
// License, Version 2.0 found in the LICENSE-APACHE file in the root directory
// of this source tree. You may select, at your option, one of the above-listed licenses.

//! A trait representing a Path ORAM stash.

use crate::{
    bucket::{Bucket, PathOramBlock},
    utils::{
        append_to_file, bitonic_sort_by_keys, create_path_if_not_exists, CompleteBinaryTreeIndex,
        TreeIndex,
    },
    Address, BucketSize, OramBlock, OramError, StashSize,
};

use subtle::{Choice, ConditionallySelectable, ConstantTimeEq};

const STASH_GROWTH_INCREMENT: usize = 10;

#[derive(Debug)]
/// A fixed-size, obliviously accessed Path ORAM stash data structure implemented using oblivious sorting.
pub struct ObliviousStash<V: OramBlock> {
    pub blocks: Vec<PathOramBlock<V>>,
    path_size: StashSize,
    m_batch: usize, // If single accesses are to be expected, let m_batch = 1.
}

impl<V: OramBlock> ObliviousStash<V> {
    fn len(&self) -> usize {
        self.blocks.len()
    }
}

impl<V: OramBlock> ObliviousStash<V> {
    pub fn new(
        path_size: StashSize,
        overflow_size: StashSize,
        m_batch: usize,
    ) -> Result<Self, OramError> {
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

        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let fixed_path_block_count: usize = usize::try_from(self.path_size)?;
        let buckets_per_path = self.path_size / u64::try_from(Z)?;

        /*
            Build the union in the canonical order required by the batched protocol:

                root first,
                then increasing depth,
                and within each level, increasing bucket id.

            This differs from the earlier "first encountered" order, which depended on
            the order of positions in the batch.
        */
        let mut union_buckets: Vec<TreeIndex> = Vec::new();
        let mut seen: HashSet<TreeIndex> = HashSet::new();

        for &position in positions {
            let height = position.ct_depth();

            for depth in 0..buckets_per_path {
                let bucket_index = position.ct_node_on_path(depth, height);

                if seen.insert(bucket_index) {
                    union_buckets.push(bucket_index);
                }
            }
        }

        union_buckets.sort_unstable_by(|a, b| {
            let da = a.ct_depth();
            let db = b.ct_depth();

            da.cmp(&db).then_with(|| a.cmp(b))
        });

        let union_block_count = union_buckets.len() * Z;

        /*
            Preserve only the real overflow stash blocks from the previous layout.

            Invariant we maintain:

                self.blocks =
                    [fixed scratch prefix of length self.path_size]
                    [real overflow stash blocks]
                    [one reserved trailing dummy block]

            During a union read, the scratch prefix may need to be temporarily larger
            than self.path_size. Instead of shifting pieces of the vector conditionally,
            rebuild the layout explicitly:

                [union scratch prefix]
                [old real overflow stash blocks]
                [reserved trailing dummy]
        */
        let overflow_blocks: Vec<PathOramBlock<V>> = self
            .blocks
            .iter()
            .skip(fixed_path_block_count)
            .copied()
            .filter(|block| !bool::from(block.ct_is_dummy()))
            .collect();

        let scratch_block_count = fixed_path_block_count.max(union_block_count);

        let mut rebuilt_blocks = vec![PathOramBlock::<V>::dummy(); scratch_block_count];

        /*
            Load the union buckets into the scratch prefix.

            The remote physical access pattern is exactly the canonical union.
        */
        for (bucket_slot, &bucket_index) in union_buckets.iter().enumerate() {
            let bucket = physical_memory[usize::try_from(bucket_index)?];

            for slot_index in 0..Z {
                rebuilt_blocks[Z * bucket_slot + slot_index] = bucket.blocks[slot_index];
            }
        }

        rebuilt_blocks.extend(overflow_blocks);

        // Preserve the library convention: one trailing dummy reserved for insertions.
        rebuilt_blocks.push(PathOramBlock::<V>::dummy());

        self.blocks = rebuilt_blocks;

        if is_log {
            let height = positions[0].ct_depth();
            let n_size = 2_usize.pow(u32::try_from(height)?);
            let log_path_name = format!("./exp-results/results/N_{}/{}", n_size, self.m_batch);
            let bandwidth_log_filename = format!("{}/{}", log_path_name, "bandwidth_batch.log");

            let _ = create_path_if_not_exists(&log_path_name);
            let _ = append_to_file(
                &bandwidth_log_filename,
                union_block_count.to_string().as_str(),
            );
        }

        Ok(union_buckets)
    }

    pub fn write_to_path_union<const Z: BucketSize>(
        &mut self,
        physical_memory: &mut [Bucket<V, Z>],
        union_buckets: &[TreeIndex],
        is_log: bool,
    ) -> Result<(), OramError> {
        use std::collections::{HashMap, HashSet};

        if union_buckets.is_empty() {
            return Ok(());
        }

        let fixed_path_block_count: usize = usize::try_from(self.path_size)?;
        let z_u64 = u64::try_from(Z)?;

        /*
            The read-side union is canonical root-to-leaf.

            For writeback, we want reverse canonical order:
                deeper buckets first,
                then shallower buckets,
                tie by increasing bucket id.

            This matches the Path ORAM greedy eviction rule: place each block as deep
            as possible among the touched buckets.
        */
        let mut seen = HashSet::new();

        let mut ordered_union_buckets: Vec<TreeIndex> = union_buckets
            .iter()
            .copied()
            .filter(|bucket| seen.insert(*bucket))
            .collect();

        ordered_union_buckets.sort_unstable_by(|a, b| {
            let da = a.ct_depth();
            let db = b.ct_depth();

            db.cmp(&da).then_with(|| a.cmp(b))
        });

        let union_bucket_count = ordered_union_buckets.len();
        let written_block_count = union_bucket_count * Z;

        /*
            Map bucket id -> slot in the writeback order.

            Slot 0 corresponds to the first bucket written, i.e. a deepest bucket.
            Since we assign blocks by walking from leaf to root, this gives the
            deepest available eligible bucket.
        */
        let bucket_index_to_slot: HashMap<TreeIndex, usize> = ordered_union_buckets
            .iter()
            .enumerate()
            .map(|(slot, &bucket_index)| (bucket_index, slot))
            .collect();

        /*
            Assignment keys used for bitonic sorting:

                0, 1, ..., union_bucket_count - 1
                    assigned to a touched bucket

                TreeIndex::MAX - 1
                    real overflow stash block

                TreeIndex::MAX
                    unused dummy block

            After sorting:

                [all blocks/dummies assigned to bucket 0]
                [all blocks/dummies assigned to bucket 1]
                ...
                [real overflow blocks]
                [unused dummies]
        */
        let mut bucket_assignments = vec![TreeIndex::MAX; self.blocks.len()];
        let mut bucket_counts = vec![0u64; union_bucket_count];

        /*
            Assign real blocks to the deepest eligible touched bucket.

            A real block with assigned leaf x can go into exactly the buckets lying
            on path(x). Since the union contains only touched buckets, we walk
            path(x) from leaf to root and choose the first touched bucket with
            available capacity.
        */
        for (block_index, block) in self.blocks.iter().enumerate() {
            if bool::from(block.ct_is_dummy()) {
                continue;
            }

            let leaf = block.position;
            let leaf_depth = leaf.ct_depth();

            let mut assigned = false;

            for depth in (0..=leaf_depth).rev() {
                let ancestor = leaf.ct_node_on_path(depth, leaf_depth);

                if let Some(&bucket_slot) = bucket_index_to_slot.get(&ancestor) {
                    if bucket_counts[bucket_slot] < z_u64 {
                        bucket_assignments[block_index] = TreeIndex::try_from(bucket_slot)?;
                        bucket_counts[bucket_slot] += 1;
                        assigned = true;
                        break;
                    }
                }
            }

            if !assigned {
                bucket_assignments[block_index] = TreeIndex::MAX - 1;
            }
        }

        /*
            Ensure enough dummy blocks exist to pad every touched bucket to size Z.

            Unlike the old loop, this computes the exact deficit first, extends once,
            then assigns dummy blocks to the remaining bucket slots.
        */
        let dummy_slots_needed: usize = bucket_counts
            .iter()
            .map(|&count| usize::try_from(z_u64 - count).unwrap_or(0))
            .sum();

        let available_dummy_count = self
            .blocks
            .iter()
            .filter(|block| bool::from(block.ct_is_dummy()))
            .count();

        if dummy_slots_needed > available_dummy_count {
            let extra = dummy_slots_needed - available_dummy_count;

            self.blocks
                .resize(self.blocks.len() + extra, PathOramBlock::<V>::dummy());

            bucket_assignments.resize(bucket_assignments.len() + extra, TreeIndex::MAX);
        }

        /*
            Assign dummy blocks to the remaining unfilled bucket slots.

            This is purely padding. It does not affect correctness of real blocks,
            but it guarantees that after sorting, the first written_block_count
            entries contain exactly Z blocks per touched bucket.
        */
        let mut next_bucket_slot = 0usize;

        for block_index in 0..self.blocks.len() {
            if next_bucket_slot >= union_bucket_count {
                break;
            }

            if !bool::from(self.blocks[block_index].ct_is_dummy()) {
                continue;
            }

            while next_bucket_slot < union_bucket_count && bucket_counts[next_bucket_slot] >= z_u64
            {
                next_bucket_slot += 1;
            }

            if next_bucket_slot >= union_bucket_count {
                break;
            }

            bucket_assignments[block_index] = TreeIndex::try_from(next_bucket_slot)?;
            bucket_counts[next_bucket_slot] += 1;
        }

        /*
            At this point every touched bucket should have exactly Z assigned blocks.
            If not, there is an internal accounting bug.
        */
        for (bucket_slot, &count) in bucket_counts.iter().enumerate() {
            if count != z_u64 {
                return Err(OramError::InvalidConfigurationError {
                    parameter_name: "union writeback bucket fill".to_string(),
                    parameter_value: format!(
                        "bucket slot {} has {} assigned blocks, expected {}",
                        bucket_slot, count, Z
                    ),
                });
            }
        }

        bitonic_sort_by_keys(&mut self.blocks, &mut bucket_assignments);

        /*
            Write back the assigned prefix.

            Since the sort keys are bucket slots, the first Z entries belong to
            ordered_union_buckets[0], the next Z entries to ordered_union_buckets[1],
            and so on.
        */
        let mut write_bandwidth = 0usize;

        for (bucket_slot, &bucket_index) in ordered_union_buckets.iter().enumerate() {
            let bucket_to_write = &mut physical_memory[usize::try_from(bucket_index)?];

            for slot_number in 0..Z {
                let stash_index = bucket_slot * Z + slot_number;
                bucket_to_write.blocks[slot_number] = self.blocks[stash_index];
            }

            write_bandwidth += Z;
        }

        /*
            Rebuild the client layout after writeback.

            All blocks written into the touched buckets must be removed from the
            client stash. Real blocks that could not be assigned to the union remain
            as overflow stash blocks.
        */
        let overflow_blocks: Vec<PathOramBlock<V>> = self
            .blocks
            .iter()
            .skip(written_block_count)
            .copied()
            .filter(|block| !bool::from(block.ct_is_dummy()))
            .collect();

        let mut rebuilt_blocks = vec![PathOramBlock::<V>::dummy(); fixed_path_block_count];

        rebuilt_blocks.extend(overflow_blocks);

        // Preserve the library convention: one trailing dummy block reserved for insertion.
        rebuilt_blocks.push(PathOramBlock::<V>::dummy());

        self.blocks = rebuilt_blocks;

        if is_log {
            let max_depth = union_buckets
                .iter()
                .map(|bucket| bucket.ct_depth())
                .max()
                .unwrap_or(0);

            let n_size = 2_usize.pow(u32::try_from(max_depth)?);
            let log_path_name = format!("./exp-results/results/N_{}/{}", n_size, self.m_batch);
            let bandwidth_log_filename = format!("{}/{}", log_path_name, "bandwidth_batch.log");
            let stash_log_filename = format!("{}/{}", log_path_name, "stash_batch.log");

            let _ = create_path_if_not_exists(&log_path_name);

            let _ = append_to_file(
                &bandwidth_log_filename,
                write_bandwidth.to_string().as_str(),
            );

            let _ = append_to_file(&stash_log_filename, self.occupancy().to_string().as_str());
        }

        Ok(())
    }

    pub fn batched_access<F: Fn(Vec<&V>) -> Vec<V>>(
        &mut self,
        addresses: Vec<Address>,
        new_positions: Vec<TreeIndex>,
        value_callback: F,
    ) -> Result<Vec<V>, OramError> {
        use subtle::Choice;

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

        /*
            For this callback interface, duplicate logical addresses are ambiguous.

            Example:
                read A, write A, read A

            Should the second read see the old value or the value written earlier
            in the same batch? Since your function applies one callback to all old
            values at once, the clean semantics require distinct addresses.
        */
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

        /*
            First pass: read old values from the client stash.

            This assumes read_from_path_union has already been called, so the stash
            currently contains all real blocks from the touched union plus any
            previous overflow stash blocks.
        */
        for block in &self.blocks {
            for request_index in 0..addresses.len() {
                let is_requested_index = block.address.ct_eq(&addresses[request_index]);

                found[request_index].conditional_assign(&1.into(), is_requested_index);
                results[request_index].conditional_assign(&block.value, is_requested_index);
            }
        }

        /*
            Apply the user-supplied batched operation.

            For a read, the callback should return the same value.
            For a write, the callback should return the replacement value.
        */
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

        /*
            Second pass: update all existing matching blocks in-place.

            The position must become the freshly sampled position, regardless of
            whether the logical operation was a read or a write.
        */
        for block in &mut self.blocks {
            for request_index in 0..addresses.len() {
                let is_requested_index = block.address.ct_eq(&addresses[request_index]);

                block
                    .position
                    .conditional_assign(&new_positions[request_index], is_requested_index);

                block
                    .value
                    .conditional_assign(&values_to_write[request_index], is_requested_index);
            }
        }

        /*
            Insert missing addresses.

            This covers the same case as ordinary Path ORAM initialization-on-first-write
            or sparse logical memory: if an address is requested but no block exists yet,
            create it in the client stash with the fresh position.
        */
        let missing_count = found.iter().filter(|choice| !bool::from(**choice)).count();

        let available_dummy_count = self
            .blocks
            .iter()
            .filter(|block| bool::from(block.ct_is_dummy()))
            .count();

        if missing_count > available_dummy_count {
            self.blocks.resize(
                self.blocks.len() + (missing_count - available_dummy_count),
                PathOramBlock::<V>::dummy(),
            );
        }

        let mut next_free_slot = self.blocks.len();

        for request_index in 0..addresses.len() {
            if bool::from(found[request_index]) {
                continue;
            }

            loop {
                if next_free_slot == 0 {
                    return Err(OramError::InvalidConfigurationError {
                        parameter_name: "stash dummy capacity".to_string(),
                        parameter_value: "no dummy block available for batch insertion".to_string(),
                    });
                }

                next_free_slot -= 1;

                if bool::from(self.blocks[next_free_slot].ct_is_dummy()) {
                    break;
                }
            }

            self.blocks[next_free_slot] = PathOramBlock {
                value: values_to_write[request_index],
                address: addresses[request_index],
                position: new_positions[request_index],
            };
        }

        Ok(results)
    }

    // Utilities for stash
    pub fn occupancy(&self) -> StashSize {
        let mut result = 0;
        for i in self.path_size.try_into().unwrap()..(self.blocks.len()) {
            if !self.blocks[i].is_dummy() {
                result += 1;
            }
        }
        result
    }

    // pub fn stash_occupancy(&self) -> StashSize {
    //     let mut result = 0;
    //     for i in 0..(self.blocks.len()) {
    //         if !self.blocks[i].is_dummy() {
    //             result += 1;
    //         }
    //     }
    //     result
    // }
}
