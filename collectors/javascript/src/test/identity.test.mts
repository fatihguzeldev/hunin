import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { plan } from '../host/plan.mjs';

for (const [runtime, executable, expected] of [
  [
    'node',
    process.execPath,
    {
      kind: 'node-cpu-prof',
      measurement_contract: 'node-cpu-prof-v1',
      completion_scope: 'declared_profile_artifacts',
    },
  ],
  [
    'deno',
    process.env.HUNIN_TEST_DENO,
    {
      kind: 'deno-cpu-prof',
      measurement_contract: 'deno-workers-v1',
      completion_scope: 'declared_profiles_and_registered_deno_workers',
    },
  ],
] as const) {
  if (!executable) continue;
  test(`${runtime} measurement identity survives planner relocation and capture sessions`, () => {
    const directory = mkdtempSync(
      join(tmpdir(), 'hunin-measurement-contract-'),
    );
    try {
      const relocated = join(directory, 'relocated-planner');
      mkdirSync(join(relocated, 'host'), { recursive: true });
      // Planning only executes these two modules. Target-agent delivery is
      // covered separately by the actual installed capture controls.
      for (const path of ['plan.mjs', 'host/plan.mjs']) {
        cpSync(
          fileURLToPath(new URL(`../${path}`, import.meta.url)),
          join(relocated, path),
        );
      }
      const request = {
        command:
          runtime === 'deno'
            ? [executable, 'eval', 'console.log(1)']
            : [executable, '-e', 'console.log(1)'],
        cwd: directory,
        artifact_dir: join(directory, 'capture-one'),
        run_id: 'capture-one',
        runtime,
      };
      const first = plan(request);
      assert.deepEqual(first.collector_identity, expected);
      const result = spawnSync(
        process.execPath,
        [join(relocated, 'plan.mjs')],
        {
          input: JSON.stringify({
            ...request,
            artifact_dir: join(directory, 'capture-two'),
            run_id: 'capture-two',
          }),
          encoding: 'utf8',
          timeout: 10000,
        },
      );
      assert.equal(result.status, 0, result.stderr);
      const moved = JSON.parse(result.stdout) as ReturnType<typeof plan>;
      assert.deepEqual(moved.collector_identity, first.collector_identity);
      if (runtime === 'node') {
        assert.notEqual(moved.env.NODE_OPTIONS, first.env.NODE_OPTIONS);
      } else {
        assert.notDeepEqual(moved.command, first.command);
        assert.notEqual(
          moved.session_identity['worker-session'],
          first.session_identity['worker-session'],
        );
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
