import numpy as np
import sys

def read_integers_from_file(filename):
    numbers = []
    try:
        with open(filename, 'r') as file:
            for line in file:
                num = int(line.strip())
                numbers.append(num)
        return numbers
    except FileNotFoundError:
        print(f"File '{filename}' not found.")
        return []
    except ValueError:
        print("File contains non-integer values.")
        return []

if __name__ == "__main__":
    filename = '../bandwidth_single.log'
    arr = np.array(read_integers_from_file(filename))
    
    if len(arr) % 2 != 0:
        sys.exit("Something went wrong. The size of the array is not even.")

    # some elements in pair to get total bandwidth from the experiment
    temp_list = []
    for i in range(0, len(arr) - 1, 2):
        temp_list.append(arr[i] + arr[i+1])

    bandwidth_arr = np.array(temp_list)

    # bandwidth read and write are fixed and equal for single accesses
    # so we sum and divide by NUM_BATCH_TEST

    print(f"Total Bandwidth: {np.sum(bandwidth_arr) / 100}")