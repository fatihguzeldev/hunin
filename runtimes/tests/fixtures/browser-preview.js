function resizePreview(input, output) {
  for (let i = 0; i < input.length; i++) output[i] = ((input[i] * 3 + 7) ^ (i & 255)) & 255;
}

globalThis.run = function preparePreviews() {
  const input = Uint8Array.from({ length: 16384 }, (_, i) => i & 255);
  const output = new Uint8Array(input.length);
  for (let batch = 0; batch < 4096; batch++) resizePreview(input, output);
  return { width: 128, height: 128, pixels: Array.from(output) };
};
