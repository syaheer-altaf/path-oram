// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is dual-licensed under either the MIT license found in the
// LICENSE-MIT file in the root directory of this source tree or the Apache
// License, Version 2.0 found in the LICENSE-APACHE file in the root directory
// of this source tree. You may select, at your option, one of the above-listed licenses.

//! A simple linear-time implementation of Oblivious RAM.

use crate::{Address, Oram, OramBlock, OramError};
use rand::{CryptoRng, RngCore};
use subtle::{ConstantTimeEq, ConstantTimeLess};

/// A simple ORAM that, for each access, ensures obliviousness by making a complete pass over the database,
/// reading and writing each memory location.
#[derive(Debug)]
pub struct LinearTimeOram<V: OramBlock> {
    /// The memory of the ORAM (public for benchmarking).
    pub physical_memory: Vec<V>,
}

impl<V: OramBlock> LinearTimeOram<V> {
    /// Returns a new `LinearTimeOram` mapping addresses `0 <= address < block_capacity` to default `V` values.
    pub fn new(block_capacity: Address) -> Result<Self, OramError> {
        log::info!("LinearTimeOram::new(capacity = {})", block_capacity,);

        let mut physical_memory = Vec::new();
        physical_memory.resize(usize::try_from(block_capacity)?, V::default());
        Ok(Self { physical_memory })
    }
}

impl<V: OramBlock> Oram for LinearTimeOram<V> {
    type V = V;

    fn access<R: RngCore + CryptoRng, F: Fn(&V) -> V>(
        &mut self,
        index: Address,
        callback: F,
        _: &mut R,
        _: bool,
    ) -> Result<V, OramError> {
        let index_in_bounds: bool = index.ct_lt(&self.block_capacity()?).into();

        // This operation is not constant-time, but only leaks whether the ORAM index is well-formed or not.
        if !index_in_bounds {
            return Err(OramError::AddressOutOfBoundsError {
                attempted: index,
                capacity: self.block_capacity()?,
            });
        }

        // This is a dummy value which will always be overwritten.
        let mut result = V::default();

        for i in 0..self.physical_memory.len() {
            let entry = &self.physical_memory[i];

            let is_requested_index = (u64::try_from(i)?).ct_eq(&index);

            result.conditional_assign(entry, is_requested_index);

            let potential_new_value = callback(entry);

            self.physical_memory[i].conditional_assign(&potential_new_value, is_requested_index);
        }
        Ok(result)
    }

    fn block_capacity(&self) -> Result<Address, OramError> {
        Ok(u64::try_from(self.physical_memory.len())?)
    }
    // `batched-access` here is the same as the `access` above (so there is literally no performance benefit here).
    // Only the `batched-access` defined in `path_oram.rs` has benefits, performance-wise.
    // The reason for implementation is due to `batched_oram` being defined in the `Oram` trait in `lib.rs`.
    fn batched_access<R: RngCore + CryptoRng, F: Fn(Vec<&Self::V>) -> Vec<Self::V>>(
        &mut self,
        indices: Vec<Address>,
        callback: F,
        _rng: &mut R,
        _is_log: bool,
    ) -> Result<Vec<Self::V>, OramError> {
        let capacity = self.block_capacity()?;

        for &address in &indices {
            let in_bounds: bool = address.ct_lt(&capacity).into();
            if !in_bounds {
                return Err(OramError::AddressOutOfBoundsError {
                    attempted: address,
                    capacity,
                });
            }
        }

        let mut results = vec![Self::V::default(); indices.len()];

        // First pass: obliviously read out all requested entries.
        for i in 0..self.physical_memory.len() {
            let entry = &self.physical_memory[i];
            let physical_index = u64::try_from(i)?;

            for (j, &address) in indices.iter().enumerate() {
                let is_requested_index = physical_index.ct_eq(&address);
                results[j].conditional_assign(entry, is_requested_index);
            }
        }

        // Feed references to the old values into the callback.
        let callback_input: Vec<&Self::V> = results.iter().collect();
        let new_values = callback(callback_input);

        if new_values.len() != indices.len() {
            return Err(OramError::InvalidConfigurationError {
                parameter_name: "callback output length".to_string(),
                parameter_value: format!("expected {}, got {}", indices.len(), new_values.len()),
            });
        }

        // Second pass: obliviously write back the updated values.
        for i in 0..self.physical_memory.len() {
            let physical_index = u64::try_from(i)?;

            for (j, &address) in indices.iter().enumerate() {
                let is_requested_index = physical_index.ct_eq(&address);
                self.physical_memory[i].conditional_assign(&new_values[j], is_requested_index);
            }
        }

        Ok(results)
    }
}