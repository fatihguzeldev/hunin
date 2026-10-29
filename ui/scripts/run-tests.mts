import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

function testSources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return testSources(path);
    return entry.isFile() && entry.name.endsWith('.test.mts') ? [path] : [];
  });
}

const uiRoot = fileURLToPath(new URL('../../', import.meta.url));
const sourceRoot = join(uiRoot, 'test');
const sources = testSources(sourceRoot).sort();
if (sources.length === 0) {
  throw new Error('No current TypeScript test sources found under ui/test.');
}
const tests = sources.map((source) =>
  join(uiRoot, '.tools', 'test', relative(sourceRoot, source)).replace(
    /\.mts$/,
    '.mjs',
  ),
);
for (const test of tests) {
  if (!existsSync(test)) {
    throw new Error(
      `Missing emitted UI test ${test}; run npm --prefix ui run build:tooling.`,
    );
  }
}
const result = spawnSync(process.execPath, ['--test', ...tests], {
  cwd: uiRoot,
  stdio: 'inherit',
});
if (result.error) throw result.error;
if (result.signal) process.kill(process.pid, result.signal);
process.exitCode = result.status ?? 1;
