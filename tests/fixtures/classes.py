class Point:
    def __init__(self, x: int, y: int):
        self.x = x
        self.y = y

    def total(self):
        return self.x + self.y


class Counter:
    def __init__(self, value: int):
        self.value = value

    def bump(self):
        self.value = self.value + 1


class First:
    def __init__(self, value: int):
        self.value = value

    def read(self):
        return self.value


class Second:
    def __init__(self, value: int):
        self.value = value
        self.second = value

    def read(self):
        return self.value


class Third:
    def __init__(self, value: int):
        self.value = value
        self.second = value
        self.third = value

    def read(self):
        return self.value


def main():
    point = Point(2, 3)
    direct = point.total()
    bound = point.total
    return direct + bound()


def loop_main():
    counter = Counter(0)
    index = 0
    while index < 20:
        counter.bump()
        index = index + 1
    return counter.value


def polymorphic_main():
    items = [First(1), Second(2), Third(3)]
    index = 0
    total = 0
    while index < 3:
        total = total + items[index].read()
        index = index + 1
    return total
