// Independent compiler fixture: two modules share names but not bytes/results.
const unsigned = (value: number): number[] => {
  const bytes: number[] = [];
  do {
    const next = value & 127;
    value = Math.floor(value / 128);
    bytes.push(next | (value ? 128 : 0));
  } while (value);
  return bytes;
};
const text = (value: string) => [
  ...unsigned(Buffer.byteLength(value)),
  ...Buffer.from(value),
];
const section = (id: number, bytes: number[]) => [
  id,
  ...unsigned(bytes.length),
  ...bytes,
];
export function moduleBytes(multiplier: number): Uint8Array<ArrayBuffer> {
  if (![3, 7].includes(multiplier))
    throw Error('fixture multiplier must be3or7');
  const kernel = [
    1,
    1,
    127,
    2,
    64,
    3,
    64,
    32,
    0,
    69,
    13,
    1,
    32,
    1,
    32,
    0,
    16,
    1,
    65,
    multiplier,
    106,
    106,
    33,
    1,
    32,
    0,
    65,
    1,
    107,
    33,
    0,
    12,
    0,
    11,
    11,
    32,
    1,
    11,
  ];
  const helper = [0, 32, 0, 65, multiplier, 108, 11];
  const names = [
    2,
    0,
    ...text('generated_kernel'),
    1,
    ...text('generated_helper'),
  ];
  return new Uint8Array([
    0,
    97,
    115,
    109,
    1,
    0,
    0,
    0,
    ...section(1, [1, 96, 1, 127, 1, 127]),
    ...section(3, [2, 0, 0]),
    ...section(7, [1, ...text('run'), 0, 0]),
    ...section(10, [
      2,
      ...unsigned(kernel.length),
      ...kernel,
      ...unsigned(helper.length),
      ...helper,
    ]),
    ...section(0, [...text('name'), 1, ...unsigned(names.length), ...names]),
  ]);
}
export function expected(iterations: number, multiplier: number): number {
  return Math.imul(
    (iterations * (iterations + 1)) / 2 + iterations,
    multiplier,
  );
}
