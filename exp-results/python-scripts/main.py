import bandwidth as b
import stash as s

import re
from pathlib import Path
from collections import defaultdict

import matplotlib.pyplot as plt
import pandas as pd

if __name__ == "__main__":
    dirs = [item for item in Path('./exp-results/results').iterdir() if item.is_dir()]
    exp_dirs = [item for item in dirs if re.match(r'exp-results/results/N', item.as_posix()) != None]
    print(exp_dirs)
    _Z = 4

    N_list = list()

    batched_bandwidths = dict()
    # single_max_stash = defaultdict(list)
    batched_max_stash = defaultdict(list)

    for _dir in exp_dirs:
        _N = int(_dir.as_posix().replace("exp-results/results/N_", ""))
        N_list.append(_N)
        batch_dirs = [item for item in _dir.iterdir() if item.is_dir()]
        
        batched_bandwidths[_N] = []

        for _batch in batch_dirs:
            batch = int(_batch.as_posix().replace(f"exp-results/results/N_{_N}/", ""))

            sb, bb, tbb, savings = b.return_bandwidths(_Z, _N, batch, f'./{_batch.as_posix()}/bandwidth_batch.log')
            bs = s.return_stash(f'./{_batch.as_posix()}/stash_batch.log')
            
            batched_bandwidths[_N].append({
                "m" : batch,
                "total-single-bandwidth": sb,
                "total-batched-bandwidth (mean)": bb,
                "theoretical value (mean)": tbb,
                "savings (%)": savings
            })
            # single_max_stash[batch].append((_N, ss))
            batched_max_stash[batch].append((_N, bs))

    pd.set_option('display.max_columns', None)

    # print comparisons in bandwidths
    N_list = sorted(N_list)
    for idx in N_list:
        data = batched_bandwidths[idx]
        data = sorted(data, key=lambda d: d['m'])
        df = pd.DataFrame(data)
        print(f"\n\nBandwidth comparisons for N = {idx}:")
        print(df.to_string(index=False))

    # try plotting stash for batched accesses
    plt.figure(figsize=(8, 5))

    for label, pairs in sorted(batched_max_stash.items()):
        # sort by x so the line is drawn in the correct order
        pairs = sorted(pairs, key=lambda p: p[0])
        x = [p[0] for p in pairs]
        y = [p[1] for p in pairs]
        
        plt.plot(x, y, marker='o', label=f'm={label}')

    plt.xscale('log', base=2)
    plt.xlabel('x')
    plt.ylabel('y')
    plt.title(f'Max Stash Growth for m-Batched Accesses, Z = {_Z}')
    plt.legend()
    plt.grid(True, which='both', linestyle='--', alpha=0.5)
    plt.tight_layout()
    plt.show()
