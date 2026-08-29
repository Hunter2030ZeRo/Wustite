# /// script
# requires-python = ">=3.13"
# dependencies = []
# ///
# How to run: cargo run --release -- bench benchmarks/compiler_kernels.py


def weighted_cell(row: int, column: int, enabled: bool):
    result = 0
    if enabled and row < column:
        result = row * column
    else:
        result = row + column
    return result


def main():
    total = 0
    for row in range(12):
        for column in range(1, 12):
            if row < column:
                total = total + row * column
            else:
                total = total + row + column

    countdown = 7
    while 0 < countdown:
        total = total + countdown
        countdown = countdown - 1

    probe = 0
    probe_limit = 8
    while probe < probe_limit:
        probe = probe + 1
    total = total + probe
    seed = weighted_cell(1, 2, True)
    return total + seed
