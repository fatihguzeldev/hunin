#!/usr/bin/env node
import { readFileSync, realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { plan } from './host/plan.mjs';

export { plan, toolPath } from './host/plan.mjs';
export type { CaptureRequest, CaptureOutput } from './host/plan.mjs';

function isEntrypoint() {
  try {
    return (
      process.argv[1] &&
      realpathSync(process.argv[1]) ===
        realpathSync(fileURLToPath(import.meta.url))
    );
  } catch {
    return false;
  }
}
if (isEntrypoint()) {
  try {
    process.stdout.write(
      JSON.stringify(plan(JSON.parse(readFileSync(0, 'utf8')))) + '\n',
    );
  } catch (error) {
    process.stderr.write(String((error as Error).message ?? error) + '\n');
    process.exitCode = 1;
  }
}
