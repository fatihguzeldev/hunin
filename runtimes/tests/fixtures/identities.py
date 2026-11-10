def run():
    first, second = (lambda value: value + 1), (lambda value: value * 3)
    return {"first": first(4), "second": second(4)}
