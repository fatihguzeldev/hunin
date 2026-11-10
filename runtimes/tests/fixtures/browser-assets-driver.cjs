'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
function input() {
  const directory = process.env.PREVIEW_INPUT;
  if (!path.isAbsolute(directory || '')) throw new Error('missing asset directory');
  const assets = [];
  for (const name of fs.readdirSync(directory).filter(x => x.endsWith('.ppm')).sort()) {
    const bytes = fs.readFileSync(path.join(directory, name));
    const header = /^P6\n(\d+) (\d+)\n255\n/.exec(bytes.subarray(0, 64).toString('ascii'));
    if (!header) throw new Error('fixture expects canonical P6 input');
    const width = Number(header[1]), height = Number(header[2]);
    if (width < 1 || height < 1 || width > 4096 || height > 4096 || bytes.length !== header[0].length + width * height * 3) throw new Error('invalid P6 fixture');
    assets.push({ name, width, height, pixels: Array.from(bytes.subarray(header[0].length)) });
  }
  const repetitions = Number(process.env.PREVIEW_REPETITIONS || 10000);
  if (!assets.length || !Number.isInteger(repetitions) || repetitions < 1 || repetitions > 100000) throw new Error('invalid prepared assets');
  return { assets, repetitions };
}
function output(result) {
  const directory = process.env.PREVIEW_OUTPUT;
  if (!path.isAbsolute(directory || '')) throw new Error('missing output directory');
  fs.mkdirSync(directory, { recursive: true });
  const assets = [];
  for (const asset of result.assets) {
    if (path.basename(asset.name) !== asset.name || !asset.name.endsWith('.ppm') || asset.pixels.length !== 32 * 32 * 3 || asset.pixels.some(x => !Number.isInteger(x) || x < 0 || x > 255)) throw new Error('invalid page result');
    const filename = asset.name.slice(0, -4) + '.preview.ppm';
    const bytes = Buffer.concat([Buffer.from('P6\n32 32\n255\n'), Buffer.from(asset.pixels)]);
    fs.writeFileSync(path.join(directory, filename), bytes);
    assets.push({ input: asset.name, output: filename, source_width: asset.width, source_height: asset.height, bytes: bytes.length, sha256: crypto.createHash('sha256').update(bytes).digest('hex') });
  }
  const manifest = { algorithm: 'nearest-neighbor-rgb8', width: 32, height: 32, assets };
  fs.writeFileSync(path.join(directory, 'manifest.json'), JSON.stringify(manifest));
  return manifest;
}
module.exports = { input, output };
