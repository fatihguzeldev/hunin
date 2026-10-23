"""A CPU control workload with no Hunin instrumentation."""


def hunin_python_hotspot(rounds):
    value = 1
    for item in range(rounds):
        value = ((value ^ item) * 1664525 + 1013904223) & 0xFFFFFFFF
    return value


if __name__ == "__main__":
    print(hunin_python_hotspot(3_000_000))
