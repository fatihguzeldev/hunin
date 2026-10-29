import test from 'node:test';
import assert from 'node:assert/strict';
import { importChoices } from '../src/import-format.js';
import type { ImportFormat } from '../src/operations.generated.js';

function descriptor(overrides: Partial<ImportFormat> = {}): ImportFormat {
  return {
    format: 'future-profiler-format',
    label: 'Future profiler',
    description: 'Retained profile bytes from a future profiler.',
    aliases: ['legacy-future-profile'],
    requires: ['A retained profile and its matching sidecars'],
    requires_existing_recording: false,
    ...overrides,
  };
}

test('import choices preserve the backend canonical parser key rather than aliases or filenames', () => {
  const [choice] = importChoices([descriptor()]) ?? [];
  assert.equal(choice?.value, 'future-profiler-format');
  assert.equal(choice?.label, 'Future profiler');
  assert.equal(choice?.disabled, false);
  assert.match(choice?.description ?? '', /legacy-future-profile/);
  assert.match(choice?.description ?? '', /matching sidecars/);
});

test('new-recording availability follows ownership metadata rather than a format name table', () => {
  const choices = importChoices([
    descriptor({ requires_existing_recording: true }),
    descriptor({ format: 'hunin-jsonl', requires_existing_recording: false }),
  ]);
  assert.equal(choices?.[0]?.disabled, true);
  assert.match(choices?.[0]?.description ?? '', /matching run IDs/);
  assert.equal(choices?.[1]?.disabled, false);
});

test('an absent legacy catalog is distinct from an authoritative empty registry', () => {
  assert.equal(importChoices(undefined), undefined);
  assert.deepEqual(importChoices([]), []);
});
