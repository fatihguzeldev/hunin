function resize(input) {
  const output = new Uint8Array(input.length);
  for (let i = 0; i < input.length; i++) output[i] = ((input[i] * 3 + 7) ^ (i & 255)) & 255;
  return output;
}

exports.run = function preparePreview() {
  const input = Uint8Array.from({ length: 65536 }, (_, i) => i & 255);
  let output;
  for (let batch = 0; batch < 4096; batch++) output = resize(input);
  return { width: 256, height: 256, batches: 4096, pixels: Array.from(output) };
};
