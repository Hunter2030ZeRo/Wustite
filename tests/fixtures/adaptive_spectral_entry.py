def scale(value: float):
    return value * 1.5


def main(size: int, seed: float):
    values = [seed] * size
    return scale(values[size - 1])
