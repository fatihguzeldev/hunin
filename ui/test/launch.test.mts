import { parseCommand } from '../src/command.js';
import { parseWorkloadOptions } from '../src/workload/options.js';
import test from 'node:test';
import assert from 'node:assert/strict';
import { launchChoices } from '../src/workload/choices.js';
import type { Capability } from '../src/api.js';

test('recording choices use backend launch routes and never promote probe executables to workload commands', () => {
  const runtime: Capability = {
    runtime: 'future-runtime',
    available: false,
    probe_status: 'missing_tool',
    executable: '/collector/probe',
    diagnostics: ['Default probe failed'],
    modes: [
      {
        mode: 'custom',
        mechanism: 'control',
        availability: 'missing_tool',
        basis: 'discovery',
        recording_verified: false,
        launch_runtime: 'backend-owned-route',
        selectable: true,
        default_executable: null,
        command_requirement: 'explicit target executable',
      },
    ],
  };
  const [choice] = launchChoices([runtime]);
  assert.equal(choice?.launchRuntime, 'backend-owned-route');
  assert.equal(choice?.defaultExecutable, null);
  assert.equal(choice?.unavailableReason, undefined);
  assert.equal(choice?.availability, 'missing_tool');
  assert.equal(choice?.recordingVerified, false);
  assert.equal(JSON.stringify(choice).includes('/collector/probe'), false);
});

test('missing legacy launch metadata and nonselectable modes cannot invent a runtime mapping', () => {
  const runtime: Capability = {
    runtime: 'go',
    available: true,
    probe_status: 'passed',
    diagnostics: [],
    modes: [
      {
        mode: 'go-trace',
        mechanism: 'tracer',
        availability: 'ready',
        basis: 'control',
        recording_verified: false,
      },
    ],
  };
  assert.equal(launchChoices([runtime])[0]?.launchRuntime, undefined);
  assert.match(
    launchChoices([runtime])[0]?.unavailableReason ?? '',
    /launch route/,
  );
  const mode = runtime.modes?.[0];
  assert.ok(mode);
  mode.launch_runtime = 'explicit-backend-route';
  mode.selectable = false;
  assert.equal(launchChoices([runtime])[0]?.launchRuntime, undefined);
  runtime.modes = [];
  assert.match(
    launchChoices([runtime])[0]?.unavailableReason ?? '',
    /collection modes/,
  );
});

test('recording commands retain literal arguments and reject ambiguous input', async () => {
  assert.deepEqual(
    parseCommand('["node", "file with spaces.js", "$(literal)", ""]'),
    ['node', 'file with spaces.js', '$(literal)', ''],
  );
  for (const invalid of [
    'node app.js',
    '[]',
    '[""]',
    '["node", 1]',
    '{}',
    'null',
  ])
    assert.throws(() => parseCommand(invalid));
});

test('workload recipes preserve exact output bytes and reject invalid environment declarations', async () => {
  const exact = parseWorkloadOptions(
    '{"MODE":"literal $(value)"}',
    '["data/input.json"]',
    '0',
    'text',
    'result\n',
  );
  assert.deepEqual(exact, {
    env: { MODE: 'literal $(value)' },
    input_files: ['data/input.json'],
    expected_exit_code: 0,
    expected_stdout: 'result\n',
  });
  assert.equal(
    parseWorkloadOptions('{}', '[]', '1', 'text', '').expected_stdout,
    '',
  );
  const hash = parseWorkloadOptions('{}', '[]', '0', 'sha256', 'A'.repeat(64));
  assert.equal(hash.expected_stdout_sha256, 'a'.repeat(64));
  assert.equal('expected_stdout' in hash, false);
  assert.equal(
    'expected_stdout' in
      parseWorkloadOptions('{}', '[]', '0', 'exit', 'ignored'),
    false,
  );
  for (const environment of ['[]', 'null', '{"EMPTY":1}', '{"A=B":"value"}'])
    assert.throws(() =>
      parseWorkloadOptions(environment, '[]', '0', 'exit', ''),
    );
  for (const files of ['{}', '[1]', '[""]'])
    assert.throws(() => parseWorkloadOptions('{}', files, '0', 'exit', ''));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '0', 'sha256', 'abc'));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '1.5', 'exit', ''));
});
