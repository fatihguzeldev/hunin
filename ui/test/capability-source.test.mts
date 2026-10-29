import test from 'node:test';
import assert from 'node:assert/strict';
import { operation } from '../src/api.js';
import { isCapabilityMode } from '../src/operations.generated.js';

test('capability discovery retains failed probes with missing optional runtime metadata', async () => {
  const original = globalThis.fetch;
  const result = {
    host: { os: 'control', arch: 'control' },
    schema_version: 1,
    runtimes: [
      {
        runtime: 'node',
        available: false,
        probe_status: 'internal_probe_failure',
        diagnostics: ['Availability is unknown'],
      },
    ],
    native_sampling_permission: {
      state: 'permission_unverified',
      error: 'Unavailable',
    },
    probe_limits: {
      timeout_seconds_per_command: 2,
      output_bytes_per_stream: 32768,
    },
    verification: 'Prerequisite discovery only',
  };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(await operation('capabilities', {}), result);
    globalThis.fetch = async () =>
      Response.json({
        result: {
          ...result,
          runtimes: [{ ...result.runtimes[0], available: 'unknown' }],
        },
      });
    await assert.rejects(
      operation('capabilities', {}),
      /invalid capabilities result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('source responses preserve nullable provenance while checking the measured-source envelope', async () => {
  const original = globalThis.fetch;
  const result = {
    path: 'runtime://script',
    content: 'retained',
    start_line: 1,
    identity_verified: true,
    source_kind: 'recorded_source_map',
    build_mapping_verified: false,
    diagnostic: 'Mapping remains unverified',
    provenance: null,
  };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(
      await operation('source.read', { run_id: 'r', path: result.path }),
      result,
    );
    globalThis.fetch = async () =>
      Response.json({ result: { ...result, start_line: 'one' } });
    await assert.rejects(
      operation('source.read', { run_id: 'r', path: result.path }),
      /invalid source.read result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('launch descriptors preserve explicit-command selection independently of prerequisite probes', () => {
  const legacy = {
    mode: 'automatic-cpu',
    mechanism: 'native sampling',
    availability: 'missing_tool',
    basis: 'discovery only',
    recording_verified: false,
  };
  assert.equal(isCapabilityMode(legacy), true);
  const mode = {
    ...legacy,
    launch_runtime: 'native',
    command_requirement: 'explicit target executable',
    default_executable: null,
    selectable: true,
  };
  assert.equal(isCapabilityMode(mode), true);
  assert.equal(isCapabilityMode({ ...mode, launch_runtime: 1 }), false);
  assert.equal(isCapabilityMode({ ...mode, default_executable: false }), false);
  assert.equal(isCapabilityMode({ ...mode, selectable: 'yes' }), false);
});
