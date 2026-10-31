import test from 'node:test';
import assert from 'node:assert/strict';
import { coverageText } from '../src/code/coverage.js';

test('coverage condenses duplicate sample totals while preserving different weights and unknowns', () => {
  assert.equal(
    coverageText({ sample_count: 0, weight: 0 }, 'samples'),
    '0 samples',
  );
  assert.equal(
    coverageText({ sample_count: 235, weight: 235 }, 'samples'),
    '235 samples',
  );
  assert.equal(
    coverageText({ sample_count: 2, weight: 6 }, 'samples'),
    '2 samples · 6 samples',
  );
  assert.equal(
    coverageText({ sample_count: 2, weight: 2 }, 'count'),
    '2 samples · 2 count',
  );
  assert.equal(
    coverageText({ sample_count: 2, weight: null }, 'samples'),
    '2 samples · Unknown weight · samples',
  );
});
