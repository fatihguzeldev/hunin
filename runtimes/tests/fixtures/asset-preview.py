import hashlib
import json
import os
from pathlib import Path


def read_image(data):
    cursor = 0
    def token():
        nonlocal cursor
        while cursor < len(data):
            if data[cursor] == 35:
                while cursor < len(data) and data[cursor] != 10:
                    cursor += 1
            elif data[cursor] in b"\t\r\n ":
                cursor += 1
            else:
                break
        start = cursor
        while cursor < len(data) and data[cursor] not in b"\t\r\n #":
            cursor += 1
        return data[start:cursor]
    if token() != b"P6":
        raise ValueError("expected P6 RGB image")
    width, height, maximum = int(token()), int(token()), int(token())
    if not (1 <= width <= 4096 and 1 <= height <= 4096 and maximum == 255):
        raise ValueError("invalid RGB8 dimensions")
    if data[cursor:cursor+2] == b"\r\n":
        cursor += 2
    elif data[cursor:cursor+1] in (b"\r", b"\n", b"\t", b" "):
        cursor += 1
    else:
        raise ValueError("missing PPM pixel delimiter")
    pixels = data[cursor:]
    if len(pixels) != width * height * 3:
        raise ValueError("unexpected PPM pixel length")
    return width, height, pixels


def resize(image):
    width, height, source = image
    output = bytearray(32 * 32 * 3)
    for y in range(32):
        for x in range(32):
            start = ((y * height // 32) * width + x * width // 32) * 3
            dest = (y * 32 + x) * 3
            output[dest:dest+3] = source[start:start+3]
    return output


def run():
    source, output = Path(os.environ["PREVIEW_INPUT"]), Path(os.environ["PREVIEW_OUTPUT"])
    repetitions = int(os.environ.get("PREVIEW_REPETITIONS", "16"))
    if not source.is_absolute() or not output.is_absolute() or not 1 <= repetitions <= 100000:
        raise ValueError("invalid preview input/output/repetitions")
    output.mkdir(parents=True, exist_ok=True)
    assets = []
    for path in sorted(source.glob("*.ppm")):
        image = read_image(path.read_bytes())
        for _ in range(repetitions):
            pixels = resize(image)
        data = b"P6\n32 32\n255\n" + pixels
        name = path.stem + ".preview.ppm"
        (output / name).write_bytes(data)
        assets.append({"input": path.name, "output": name, "source_width": image[0], "source_height": image[1],
                       "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    if not assets:
        raise ValueError("preview directory has no PPM assets")
    manifest = {"algorithm": "nearest-neighbor-rgb8", "width": 32, "height": 32, "assets": assets}
    (output / "manifest.json").write_text(json.dumps(manifest))
    return manifest
