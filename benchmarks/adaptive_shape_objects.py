from __future__ import annotations


class Point:
    def __init__(self, x: int, y: int):
        self.x = x
        self.y = y

    def total(self):
        return self.x + self.y


def main():
    total = 0
    index = 0
    while index < 64:
        point = Point(index, index + 1)
        total = total + point.total()
        index = index + 1
    return total
