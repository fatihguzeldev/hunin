import test from 'node:test';
import assert from 'node:assert/strict';
import { sourcePresentation } from '../src/source/presentation.js';
import type { OperationOutputs } from '../src/operations.generated.js';

const retained: OperationOutputs['source.read'] = {
  path: '/project/main.ts',
  content: 'run();',
  start_line: 1,
  total_lines: 1,
  next_line: null,
  identity_verified: true,
  source_kind: 'recorded_snapshot',
  build_mapping_verified: false,
  diagnostic: 'Saved source bytes; build mapping is unverified.',
};

test('retained bytes and source-to-build verification remain separate', () => {
  const view = sourcePresentation(retained);
  assert.equal(view.origin, 'Saved source');
  assert.equal(view.identity, 'Bytes verified');
  assert.equal(view.mapping, 'Build mapping unverified');
  assert.equal(view.content, 'lines');
  assert.equal(view.tone, '');
  assert.equal(
    sourcePresentation({ ...retained, build_mapping_verified: true }).mapping,
    'Build mapping verified',
  );
});

test('current or unknown source never inherits retained-source confidence', () => {
  const current = sourcePresentation({
    ...retained,
    identity_verified: false,
    source_kind: 'current_file',
  });
  assert.equal(current.origin, 'Current file');
  assert.equal(current.identity, 'Bytes unverified');
  assert.equal(current.tone, 'warn');
  assert.match(current.note, /may have changed/);
  assert.equal(
    sourcePresentation({ ...retained, source_kind: 'future_source_kind' })
      .origin,
    'Source',
  );
  const verifiedCurrent = sourcePresentation({
    ...retained,
    source_kind: 'current_file',
  });
  assert.match(verifiedCurrent.note, /^Current bytes are verified\./);
  assert.doesNotMatch(verifiedCurrent.note, /Saved/);
  assert.match(
    sourcePresentation({ ...retained, source_kind: 'future_source_kind' }).note,
    /^Source bytes are verified\./,
  );
});

test('empty files, blank lines, out-of-range windows and unknown windows stay distinct', () => {
  assert.equal(
    sourcePresentation({ ...retained, content: '', total_lines: 0 }).content,
    'empty-file',
  );
  assert.equal(
    sourcePresentation({ ...retained, content: '', total_lines: 1 }).content,
    'lines',
  );
  assert.equal(
    sourcePresentation({
      ...retained,
      content: '',
      start_line: 501,
      total_lines: 10,
    }).content,
    'outside-window',
  );
  assert.equal(
    sourcePresentation({ ...retained, content: '', total_lines: undefined })
      .content,
    'unknown-window',
  );
});
