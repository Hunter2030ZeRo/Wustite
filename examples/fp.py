def main():
    x = 1.0
    acc = 1.0
    i = 0

    while i < 1_000_000:
        x = x * 1.000001 + 0.000001
        acc = acc + x
        i += 1

    return acc
