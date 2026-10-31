import test from 'node:test';
import assert from 'node:assert/strict';
import { hexBytes } from '../src/code/proof.js';

test('binary proof excerpts keep exact bytes and absolute hexadecimal offsets', () => {
  assert.equal(hexBytes('00ff80e282ac', 0x10), '0x10  00 ff 80 e2 82 ac');
  assert.equal(
    hexBytes('00'.repeat(17), 0x100),
    `0x100  ${Array(16).fill('00').join(' ')}\n0x110  00`,
  );
  assert.equal(hexBytes('', 0), '');
  assert.throws(() => hexBytes('0', 0));
  assert.throws(() => hexBytes('not bytes', 0));
});
