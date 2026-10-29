import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const uiRoot = fileURLToPath(new URL('../../', import.meta.url));
const compiler = createRequire(import.meta.url).resolve('typescript/bin/tsc');

test('UI runner executes current TypeScript tests and rejects orphan emissions', () => {
  const root = mkdtempSync(join(tmpdir(), 'hunin-ui-test-inventory-'));
  try {
    const sources = join(root, 'test');
    mkdirSync(sources);
    const current = join(sources, 'current.test.mts');
    const removed = join(sources, 'removed.test.mts');
    writeFileSync(
      current,
      "import { writeFileSync } from 'node:fs';\n" +
        "writeFileSync(new URL('../../current-ran', import.meta.url), 'ran');\n",
    );
    writeFileSync(
      removed,
      "import { writeFileSync } from 'node:fs';\n" +
        "writeFileSync(new URL('../../orphan-ran', import.meta.url), 'ran');\n" +
        "throw new Error('obsolete test must never execute');\n",
    );
    const compiled = spawnSync(
      process.execPath,
      [
        compiler,
        '--rootDir',
        root,
        '--outDir',
        join(root, '.tools'),
        '--target',
        'ES2022',
        '--module',
        'NodeNext',
        '--moduleResolution',
        'NodeNext',
        '--types',
        'node',
        '--typeRoots',
        join(uiRoot, '..', 'node_modules', '@types'),
        '--strict',
        '--noEmitOnError',
        '--skipLibCheck',
        current,
        removed,
      ],
      { encoding: 'utf8' },
    );
    assert.equal(compiled.status, 0, compiled.stdout + compiled.stderr);
    unlinkSync(removed);
    const currentOutput = join(root, '.tools/test/current.test.mjs');
    const orphanOutput = join(root, '.tools/test/removed.test.mjs');
    const environment = { ...process.env, NODE_TEST_CONTEXT: undefined };
    const staleGlob = spawnSync(
      process.execPath,
      ['--test', currentOutput, orphanOutput],
      { encoding: 'utf8', env: environment },
    );
    assert.notEqual(staleGlob.status, 0);
    assert.equal(existsSync(join(root, 'orphan-ran')), true);
    unlinkSync(join(root, 'current-ran'));
    unlinkSync(join(root, 'orphan-ran'));
    const emittedScripts = join(root, '.tools', 'scripts');
    mkdirSync(emittedScripts);
    const runner = join(emittedScripts, 'run-tests.mjs');
    copyFileSync(join(uiRoot, '.tools', 'scripts', 'run-tests.mjs'), runner);
    const invoke = () =>
      spawnSync(process.execPath, [runner], {
        cwd: tmpdir(),
        encoding: 'utf8',
        // Model the CLI entrypoint outside this test worker's IPC context.
        env: environment,
      });
    const passed = invoke();
    assert.equal(passed.status, 0, passed.stdout + passed.stderr);
    assert.equal(existsSync(join(root, 'current-ran')), true);
    assert.equal(existsSync(join(root, 'orphan-ran')), false);
    assert.equal(existsSync(orphanOutput), true);

    unlinkSync(currentOutput);
    const missing = invoke();
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /Missing emitted UI test/);
    assert.equal(existsSync(join(root, 'orphan-ran')), false);

    unlinkSync(current);
    const empty = invoke();
    assert.notEqual(empty.status, 0);
    assert.match(empty.stderr, /No current TypeScript test sources/);
    assert.equal(existsSync(join(root, 'orphan-ran')), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
