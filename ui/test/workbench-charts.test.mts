import test from 'node:test';
import assert from 'node:assert/strict';
import {
  binAt,
  binGeometry,
  stackScope,
} from '../src/investigation/workbench/geometry.js';

test('unequal time bins occupy their actual timestamp interval on a zoomed axis', () => {
  const domain = { start_us: 0, end_us: 121 };
  assert.deepEqual(
    binGeometry({ start_us: 0, end_us: 1, weight: 1, sample_count: 1 }, domain),
    { left: 0, width: 100 / 121, unavailable: false },
  );
  const last = binGeometry(
    { start_us: 119, end_us: 121, weight: 1, sample_count: 1 },
    domain,
  );
  assert.equal(last.left, (119 / 121) * 100);
  assert.equal(last.width, (2 / 121) * 100);
  assert.equal(
    binAt([{ start_us: 0 }, { start_us: 10 }, { start_us: 90 }], 85),
    1,
  );
  assert.equal(
    binAt([{ start_us: 0 }, { start_us: 10 }, { start_us: 90 }], 90),
    2,
  );
});

test('unavailable measured-bin weight is distinct from a measured zero', () => {
  const domain = { start_us: 0, end_us: 100 };
  assert.equal(
    binGeometry(
      { start_us: 0, end_us: 100, weight: null, sample_count: 3 },
      domain,
    ).unavailable,
    true,
  );
  assert.equal(
    binGeometry(
      { start_us: 0, end_us: 100, weight: 0, sample_count: 0 },
      domain,
    ).unavailable,
    false,
  );
});

test('bounded-away root paths retain their share of the complete selected-window weight', () => {
  assert.deepEqual(stackScope(150, [100], false), {
    denominator: 150,
    omitted: 50,
    focused: false,
  });
  assert.deepEqual(stackScope(150, [100], true), {
    denominator: 100,
    omitted: 0,
    focused: true,
  });
  assert.deepEqual(stackScope(null, [100], false), {
    denominator: null,
    omitted: null,
    focused: false,
  });
});
