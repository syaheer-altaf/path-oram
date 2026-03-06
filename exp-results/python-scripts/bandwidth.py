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

def return_bandwidths(Z, N, batch, b_batch_file):
    '''
    Returns total single bandwidths, mean batched bandwidths, theoretical mean batched bandwidths,
    and savings.
    '''
    ################################################
    # Compute (total) bandwidth for single accesses
    ################################################

    total_single_bandwidth = (math.log2(N) + 1) * Z * batch

    ################################################################
    # Compute (total) bandwidth for batched accesses from log file #
    ################################################################

    bandwidth_batch_file = b_batch_file
    arr_bandwidth_batch = np.array(u.read_integers_from_file(bandwidth_batch_file))
    
    if len(arr_bandwidth_batch) % 2 != 0:
        sys.exit("Something went wrong. The size of the array is not even.")

    # Sum elements in pair to get total bandwidth from the experiment.
    # Because each m-batched experiment has a single read and a single write.
    temp_list = []
    for i in range(0, len(arr_bandwidth_batch) - 1, 2):
        temp_list.append(arr_bandwidth_batch[i] + arr_bandwidth_batch[i+1])

    bandwidth_arr = np.array(temp_list)

    savings = 100 * (total_single_bandwidth - np.mean(bandwidth_arr)) / total_single_bandwidth

    return total_single_bandwidth, np.mean(bandwidth_arr), expected_bandwidth_for_m_batches(batch, Z, N), savings
