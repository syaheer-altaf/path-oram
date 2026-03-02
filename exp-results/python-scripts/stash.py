import numpy as np
import math
import sys

import utils as u

if __name__ == "__main__":
    stash_single_file = '../stash_single.log'
    arr_single_stash = np.array(u.read_integers_from_file(stash_single_file))

    stash_batch_file = '../stash_batch.log'
    arr_batch_stash = np.array(u.read_integers_from_file(stash_batch_file))

    print("=" * 50)
    print(f"Max Stash Single: {np.max(arr_single_stash)}")
    print("=" * 50)

    print("=" * 50)
    print(f"Max Stash Batched: {np.max(arr_batch_stash)}")
    print("=" * 50)