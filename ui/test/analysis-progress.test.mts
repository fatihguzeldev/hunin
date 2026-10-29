import test from 'node:test';
import assert from 'node:assert/strict';
import { AnalysisProgress } from '../src/analysis-progress.js';

test('analyzer refresh follows an observed attempt transition, never a missing paginated row', () => {
  const progress = new AnalysisProgress();
  assert.equal(
    progress.observe([{ id: 'historical', status: 'accepted' }]),
    false,
  );
  assert.equal(progress.observe([{ id: 'active', status: 'running' }]), false);
  assert.equal(
    progress.observe([{ id: 'older-page', status: 'accepted' }]),
    false,
  );
  assert.equal(progress.observe([]), false);
  assert.equal(progress.observe([{ id: 'active', status: 'accepted' }]), true);
  assert.equal(progress.observe([{ id: 'active', status: 'accepted' }]), false);
});

test('terminal cancellation or extension statuses refresh once without inferring publication or inventing identity', () => {
  const progress = new AnalysisProgress();
  progress.observe([
    { id: 'cancel', status: 'running' },
    { id: 'extension', status: 'running' },
  ]);
  assert.equal(progress.observe([{ status: 'accepted' }]), false);
  assert.equal(progress.observe([{ id: 'cancel', status: 'cancelled' }]), true);
  assert.equal(
    progress.observe([{ id: 'extension', status: 'historical-extension' }]),
    true,
  );
});
