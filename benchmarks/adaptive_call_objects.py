from __future__ import annotations


class Amplifier:
    def apply(self, value: int):
        return value * 3 + 1


def main():
    amplifier = Amplifier()
    total = 0
    index = 0
    while index < 128:
        total = total + amplifier.apply(index)
        index = index + 1
    return total
