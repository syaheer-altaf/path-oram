import numpy as np
import math
import sys

import utils as u

def return_stash(s_batch_file):
    '''
    Returns max usage for stash singles and batched.
    '''
    # stash_single_file = s_single_file
    # arr_single_stash = np.array(u.read_integers_from_file(stash_single_file))

    stash_batch_file = s_batch_file
    arr_batch_stash = np.array(u.read_integers_from_file(stash_batch_file))

    return np.max(arr_batch_stash)