#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { plan } from './host/plan.mjs';

try {
  process.stdout.write(
    JSON.stringify(plan(JSON.parse(readFileSync(0, 'utf8')))) + '\n',
  );
} catch (error) {
  process.stderr.write(String((error as Error).message ?? error) + '\n');
  process.exitCode = 1;
}
