'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
function readImage(bytes) {
  let cursor = 0;
  function token() {
    while (cursor < bytes.length) {
      if (bytes[cursor] === 35) { while (cursor < bytes.length && bytes[cursor] !== 10) cursor++; }
      else if ([9,10,13,32].includes(bytes[cursor])) cursor++;
      else break;
    }
    const start = cursor;
    while (cursor < bytes.length && ![9,10,13,32,35].includes(bytes[cursor])) cursor++;
    return bytes.subarray(start,cursor).toString('ascii');
  }
  if (token() !== 'P6') throw new Error('expected P6 RGB image');
  const width = Number(token()), height = Number(token()), max = Number(token());
  if (!Number.isInteger(width) || !Number.isInteger(height) || width<1 || height<1 || width>4096 || height>4096 || max!==255) throw new Error('invalid RGB8 dimensions');
  if (bytes[cursor] === 13 && bytes[cursor+1] === 10) cursor+=2;
  else if ([9,10,13,32].includes(bytes[cursor])) cursor++;
  else throw new Error('missing PPM pixel delimiter');
  const pixels=bytes.subarray(cursor);
  if (pixels.length!==width*height*3) throw new Error('unexpected PPM pixel length');
  return {width,height,pixels};
}
function resize(image) {
  const output=Buffer.alloc(32*32*3);
  for(let y=0;y<32;y++) for(let x=0;x<32;x++) {
    const from=(Math.floor(y*image.height/32)*image.width+Math.floor(x*image.width/32))*3;
    const to=(y*32+x)*3;
    output[to]=image.pixels[from];output[to+1]=image.pixels[from+1];output[to+2]=image.pixels[from+2];
  }
  return output;
}
function run() {
  const input=process.env.PREVIEW_INPUT, output=process.env.PREVIEW_OUTPUT;
  const repetitions=Number(process.env.PREVIEW_REPETITIONS || 10000);
  if(!path.isAbsolute(input||'') || !path.isAbsolute(output||'') || !Number.isInteger(repetitions) || repetitions<1 || repetitions>100000) throw new Error('invalid preview input/output/repetitions');
  fs.mkdirSync(output,{recursive:true});
  const assets=[];
  for(const name of fs.readdirSync(input).filter(name=>name.endsWith('.ppm')).sort()) {
    const image=readImage(fs.readFileSync(path.join(input,name)));
    let pixels;
    for(let pass=0;pass<repetitions;pass++) pixels=resize(image);
    const bytes=Buffer.concat([Buffer.from('P6\n32 32\n255\n'),pixels]);
    const filename=name.slice(0,-4)+'.preview.ppm';
    fs.writeFileSync(path.join(output,filename),bytes);
    assets.push({input:name,output:filename,source_width:image.width,source_height:image.height,bytes:bytes.length,sha256:crypto.createHash('sha256').update(bytes).digest('hex')});
  }
  if(!assets.length) throw new Error('preview directory has no PPM assets');
  const manifest={algorithm:'nearest-neighbor-rgb8',width:32,height:32,assets};
  fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest));
  return manifest;
}
module.exports={run,resize};
