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