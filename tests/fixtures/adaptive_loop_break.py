def main():
    total = 0
    index = 0
    while index < 180:
        if 119 < index:
            break
        total = total + index
        index = index + 1
    return total
