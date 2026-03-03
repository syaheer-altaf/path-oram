import numpy as np
import math
import sys

import utils as u

# Compute closed-form expected bandwidth for m-batched accesses to a complete binary tree for bucket size Z.
def expected_bandwidth_for_m_batches(m, Z, num_leaves):
    height = int(math.log2(num_leaves))
    expectation = 0
    for i in range(height + 1):
        a = 1 - math.pow(2, -i)
        b = math.pow(a, m)
        term = math.pow(2, i) * (1 - b)
        expectation += term
    
    return (2 * Z * expectation)
if __name__ == "__main__":
    NUM_BATCH_TEST = 100
    ###############################################################
    # Compute (total) bandwidth for single accesses from log file #
    ###############################################################

    bandwidth_single_file = '../bandwidth_single.log'
    arr_bandwidth_single = np.array(u.read_integers_from_file(bandwidth_single_file))

    if len(arr_bandwidth_single) % 2 != 0:
        sys.exit("Something went wrong. The size of the array is not even.")

    # Bandwidth read and write are fixed and equal for single accesses.
    # So we sum and divide by NUM_BATCH_TEST.
    total_single_bandwidth = np.sum(arr_bandwidth_single) / NUM_BATCH_TEST
    print("=" * 50)
    print(f"Total bandwidth for single accesses: {total_single_bandwidth}")
    print("=" * 50)

    ################################################################
    # Compute (total) bandwidth for batched accesses from log file #
    ################################################################

    bandwidth_batch_file = '../bandwidth_batch.log'
    arr_bandwidth_batch = np.array(u.read_integers_from_file(bandwidth_batch_file))
    
    if len(arr_bandwidth_batch) % 2 != 0:
        sys.exit("Something went wrong. The size of the array is not even.")

    # Sum elements in pair to get total bandwidth from the experiment.
    # Because each m-batched experiment has a single read and a single write.
    temp_list = []
    for i in range(0, len(arr_bandwidth_batch) - 1, 2):
        temp_list.append(arr_bandwidth_batch[i] + arr_bandwidth_batch[i+1])

    bandwidth_arr = np.array(temp_list)

    print("=" * 50)
    print(f"Total Bandwidth (Mean): {np.mean(bandwidth_arr)}")
    print(f"Total Bandwidth (std deviation): {np.std(bandwidth_arr)}")
    print(f"\nTheoretical Total Bandwidth (Mean): {expected_bandwidth_for_m_batches(8, 4, 512)}")
    print("=" * 50)
