import test from 'node:test';
import assert from 'node:assert/strict';
import { launchChoices } from '../src/launch.js';
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
