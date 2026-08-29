def main():
    total = 0
    index = 0
    while index < 130:
        if index < 60:
            total = total + 2
        else:
            total = total + 3
        index = index + 1
    return total
