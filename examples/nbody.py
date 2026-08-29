# Adapted from PyPerformance's bm_nbody benchmark.
# Source: https://github.com/python/pyperformance

PI = 3.14159265358979323
SOLAR_MASS = 4 * PI * PI
DAYS_PER_YEAR = 365.24
DEFAULT_ITERATIONS = 20000


def combinations(items: list):
    result = []
    for x in range(len(items) - 1):
        tail = items[x + 1 :]
        for y in tail:
            result.append((items[x], y))
    return result


def advance(dt: float, n: int, bodies: list, pairs: list):
    for _ in range(n):
        for body1, body2 in pairs:
            (x1, y1, z1), v1, m1 = body1
            (x2, y2, z2), v2, m2 = body2
            dx = x1 - x2
            dy = y1 - y2
            dz = z1 - z2
            mag = dt * ((dx * dx + dy * dy + dz * dz) ** (-1.5))
            b1m = m1 * mag
            b2m = m2 * mag
            v1[0] -= dx * b2m
            v1[1] -= dy * b2m
            v1[2] -= dz * b2m
            v2[0] += dx * b1m
            v2[1] += dy * b1m
            v2[2] += dz * b1m
        for r, velocity, _ in bodies:
            vx, vy, vz = velocity
            r[0] += dt * vx
            r[1] += dt * vy
            r[2] += dt * vz
    return 0


def report_energy(bodies: list, pairs: list):
    energy = 0.0
    for body1, body2 in pairs:
        (x1, y1, z1), _, m1 = body1
        (x2, y2, z2), _, m2 = body2
        dx = x1 - x2
        dy = y1 - y2
        dz = z1 - z2
        energy -= (m1 * m2) / ((dx * dx + dy * dy + dz * dz) ** 0.5)
    for _, velocity, mass in bodies:
        vx, vy, vz = velocity
        energy += mass * (vx * vx + vy * vy + vz * vz) / 2.0
    return energy


def offset_momentum(reference: tuple, bodies: list):
    px = 0.0
    py = 0.0
    pz = 0.0
    for _, velocity, mass in bodies:
        vx, vy, vz = velocity
        px -= vx * mass
        py -= vy * mass
        pz -= vz * mass
    _, velocity, mass = reference
    velocity[0] = px / mass
    velocity[1] = py / mass
    velocity[2] = pz / mass
    return 0


def main():
    bodies = [
        ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], SOLAR_MASS),
        (
            [4.841431442464721, -1.1603200440274284, -0.1036220444711231],
            [
                0.001660076642744037 * DAYS_PER_YEAR,
                0.007699011184197404 * DAYS_PER_YEAR,
                -0.0000690460016972063 * DAYS_PER_YEAR,
            ],
            0.0009547919384243266 * SOLAR_MASS,
        ),
        (
            [8.34336671824458, 4.124798564124305, -0.4035234171143214],
            [
                -0.002767425107268624 * DAYS_PER_YEAR,
                0.004998528012349172 * DAYS_PER_YEAR,
                0.000023041729757376393 * DAYS_PER_YEAR,
            ],
            0.0002858859806661308 * SOLAR_MASS,
        ),
        (
            [12.894369562139131, -15.111151401698631, -0.22330757889265573],
            [
                0.002964601375647616 * DAYS_PER_YEAR,
                0.0023784717395948095 * DAYS_PER_YEAR,
                -0.000029658956854023756 * DAYS_PER_YEAR,
            ],
            0.00004366244043351563 * SOLAR_MASS,
        ),
        (
            [15.379697114850917, -25.919314609987964, 0.17925877295037118],
            [
                0.0026806777249038932 * DAYS_PER_YEAR,
                0.001628241700382423 * DAYS_PER_YEAR,
                -0.00009515922545197159 * DAYS_PER_YEAR,
            ],
            0.000051513890204661145 * SOLAR_MASS,
        ),
    ]
    pairs = combinations(bodies)
    offset_momentum(bodies[0], bodies)
    advance(0.01, DEFAULT_ITERATIONS, bodies, pairs)
    return report_energy(bodies, pairs)
