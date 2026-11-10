function resizePreview(image) {
  const pixels = new Array(32 * 32 * 3);
  for (let y = 0; y < 32; y++) for (let x = 0; x < 32; x++) {
    const from = (Math.floor(y * image.height / 32) * image.width + Math.floor(x * image.width / 32)) * 3;
    const to = (y * 32 + x) * 3;
    pixels[to] = image.pixels[from];
    pixels[to + 1] = image.pixels[from + 1];
    pixels[to + 2] = image.pixels[from + 2];
  }
  return pixels;
}
async function preparePreviews(input) {
  const assets = [];
  for (const image of input.assets) {
    let pixels;
    for (let pass = 0; pass < input.repetitions; pass++) pixels = resizePreview(image);
    assets.push({ name: image.name, width: image.width, height: image.height, pixels });
  }
  return { assets };
}
globalThis.run = preparePreviews;
