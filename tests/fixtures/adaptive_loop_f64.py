def main():
    total = 0.0
    value = 1.5
    flag = True
    index = 0
    while index < 120:
        if value >= 1.0:
            total = total + value
        else:
            total = total + 0.0
        value = -value
        value = -value
        value = value / 1.0
        flag = not flag
        flag = flag or False
        flag = not flag
        flag = flag and True
        index = index + 1
    return total
