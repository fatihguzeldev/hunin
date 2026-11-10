export function resize(input: Uint8Array): number[] {
  const output: number[] = new Array(input.length);
  for (let i = 0; i < input.length; i++) {
    output[i] = ((input[i] * 3 + 7) ^ input[i]) & 255;
  }
  return output;
}
export function run(): { pixels: number[]; width: number; height: number } {
  const input = Uint8Array.from({ length: 256 * 256 }, (_, i) => i & 255);
  let output: number[] = [];
  for (let round = 0; round < 1024; round++) output = resize(input);
  return { pixels: output, width: 256, height: 256 };
}
