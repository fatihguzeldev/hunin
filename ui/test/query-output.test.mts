import {
  observationLabel,
  referenceIds,
  nativeEvidenceRows,
} from '../src/observation.js';
import type { Observation } from '../src/record.generated.js';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { operation } from '../src/api.js';
import { numeric, duration, bytes, percentage } from '../src/format.js';
import { weight, weightShare, totalWeight } from '../src/profile-weight.js';

test('observation responses enforce the canonical record shape at precision and variant boundaries', async () => {
  const fixtures: {
    cases: { name: string; valid: boolean; record: unknown }[];
  } = JSON.parse(
    await readFile(
      new URL(
        '../../../protocol/fixtures/record-response-cases.json',
        import.meta.url,
      ),
      'utf8',
    ),
  );
  const original = globalThis.fetch;
  try {
    for (const fixture of fixtures.cases) {
      globalThis.fetch = async () =>
        Response.json({ result: { records: [fixture.record] } });
      const result = operation('evidence.get', {
        run_id: 'recording',
        ids: ['event'],
      });
      if (fixture.valid) await result;
      else
        await assert.rejects(
          result,
          /invalid evidence.get result/,
          fixture.name,
        );
    }
  } finally {
    globalThis.fetch = original;
  }
});

test('unknown aggregate weights are not presented as measured zero', () => {
  assert.equal(numeric(null), '—');
  assert.match(weight(null, 'samples'), /Unknown/);
  assert.equal(weight(0, 'samples'), '0 samples');
  assert.equal(weightShare(null, 10), undefined);
  assert.equal(weightShare(5, 10), 50);
  assert.equal(totalWeight([1, null]), null);
  assert.equal(totalWeight([Number.MAX_VALUE, Number.MAX_VALUE]), null);
  assert.equal(totalWeight([1, 2]), 3);
});

test('recursive tree nodes keep explicit null positions and validate nested children', async () => {
  const original = globalThis.fetch;
  const node = {
    id: 'frame',
    name: 'hotspot',
    file: null,
    line: null,
    module: null,
    code_id: null,
    source_position_basis: 'sampled',
    self_weight: null,
    total_weight: null,
    child_count: 0,
    children_truncated: false,
    depth_truncated: false,
    children: [],
  };
  const result = {
    roots: [node],
    weight_unit: 'samples',
    total_weight: null,
    scope_self_weight: null,
    path: [],
    returned_nodes: 1,
    root_count: 1,
    next_cursor: null,
    node_limit: 10,
    depth_limit: 2,
    diagnostics: [],
  };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(
      await operation('profiles.tree', { run_id: 'recording' }),
      result,
    );
    globalThis.fetch = async () =>
      Response.json({
        result: {
          ...result,
          roots: [{ ...node, children: [{ ...node, name: 7 }] }],
        },
      });
    await assert.rejects(
      operation('profiles.tree', { run_id: 'recording' }),
      /invalid profiles.tree result/,
    );
    const event = {
      schema_version: 1,
      kind: 'event',
      id: 'event',
      run_id: 'recording',
      producer_id: 'control',
      pid: 1,
      tid: 'thread',
      timestamp_us: 1,
      basis: 'observed',
      name: 'event',
    };
    globalThis.fetch = async () =>
      Response.json({
        result: { contexts: [event], relations: [], next_cursor: null },
      });
    await assert.rejects(
      operation('contexts.query', { run_id: 'recording' }),
      /invalid contexts.query result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('weight formatting keeps sample counts separate from elapsed durations', async () => {
  assert.equal(duration(1_000_000), '1 ms');
  assert.equal(duration(1_500_000_000), '1.5 s');
  assert.equal(bytes(1024), '1 KiB');
  assert.equal(percentage(null), '—');
  assert.equal(percentage(undefined), '—');
  assert.equal(percentage(0), '0%');
  assert.equal(percentage(50), '+50%');
});

test('native lifetime labels retain instance identity and unknown mapping evidence', async () => {
  const first: Observation = {
    schema_version: 1,
    kind: 'event',
    id: 'module',
    run_id: 'r',
    producer_id: 'p',
    tid: '7',
    timestamp_us: 0,
    basis: 'observed',
    name: 'module.observed_lifetime',
    pid: 7,
    attributes: {
      scope: 'module',
      module: '/tmp/libA.dylib',
      process_instance_id: 'p1',
      mapping_instance_id: 'load1',
      start_censored: true,
      end_censored: false,
      start_window_ms: [null, 3],
      exact_event_timestamps: false,
    },
  };
  const again = {
    ...first,
    attributes: { ...first.attributes, mapping_instance_id: 'load3' },
  };
  assert.equal(observationLabel(first), 'libA.dylib · p1/load1');
  assert.notEqual(observationLabel(first), observationLabel(again));
  assert.ok(
    nativeEvidenceRows(first).some(
      ([name, value]) =>
        name.startsWith('Start window') && value === '[null,3]',
    ),
  );
  assert.ok(
    nativeEvidenceRows(first).some(
      ([name, value]) => name === 'Exact event timestamps' && value === 'false',
    ),
  );
  const sample: Observation = {
    ...first,
    name: 'mapping-control',
    evidence: ['raw'],
    attributes: {
      native_load_instances: [
        { record_id: 'module-1', load_id: 'load1' },
        { record_id: null, reason: 'mapping_unresolved' },
        { record_id: 'module-1' },
      ],
    },
  };
  assert.deepEqual(referenceIds(sample), ['raw', 'module-1']);
  assert.ok(
    nativeEvidenceRows(sample).some(([, value]) =>
      value.includes('mapping_unresolved'),
    ),
  );
  assert.deepEqual(
    nativeEvidenceRows({
      ...first,
      name: 'mapping-control',
      attributes: { native_mapping_status: 'sample_link_unavailable' },
    }),
    [['Native mapping status', 'sample_link_unavailable']],
  );
});
