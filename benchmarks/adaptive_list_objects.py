from __future__ import annotations


def main():
    values = []
    index = 0
    while index < 64:
        values.append(index)
        index = index + 1

    index = 0
    while index < 32:
        values.insert(0, values.pop())
        index = index + 1

    total = 0
    for value in values:
        total = total + value
    return total
