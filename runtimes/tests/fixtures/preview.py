def resize(data):
    return bytes(((value * 3 + 7) ^ (i & 255)) & 255 for i, value in enumerate(data))


def run():
    data = bytes(i & 255 for i in range(4096))
    for _ in range(7):
        output = resize(data)
    return {"width": 64, "height": 64, "batches": 7, "pixels": list(output)}
