def main():
    acc = 0
    index = 1
    step = 1
    limit = 1000001

    while index < limit:
        acc = acc + index
        index = index + step

    return acc