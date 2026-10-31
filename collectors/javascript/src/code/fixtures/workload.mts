import { moduleBytes, expected } from './emitter.mjs';
const kernels = (process.argv[3] === 'single' ? [3] : [3, 7]).map((multiplier) => ({
  multiplier,
  run: new WebAssembly.Instance(new WebAssembly.Module(moduleBytes(multiplier)))
    .exports.run as (n: number) => number,
}));
const iterations = 200_000;
const until = performance.now() + Number(process.argv[2] ?? 1500);
let runs = 0;
while (performance.now() < until) {
  for (const kernel of kernels) {
    const actual = kernel.run(iterations);
    if (actual !== expected(iterations, kernel.multiplier))
      throw Error(`fixture checksum mismatch ${actual}`);
  }
  runs++;
}
console.log(
  JSON.stringify({
    fixture: 'independent-wasm-emitter-v1',
    runs,
    checksums: kernels.map((k) => expected(iterations, k.multiplier)),
  }),
);
