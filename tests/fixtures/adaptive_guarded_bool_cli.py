def identity(flag: bool):
    return flag

def main():
    index = 0
    value = True
    while index < 97:
        value = identity(True)
        index = index + 1
    index = 0
    while index < 33:
        value = identity(False)
        index = index + 1
    return value
