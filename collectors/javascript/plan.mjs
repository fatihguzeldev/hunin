#!/usr/bin/env node
import { mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { resolve, basename } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

export function plan(request) {
  const { command, cwd, artifact_dir, run_id } = request;
  if (!Array.isArray(command) || !command.length || command.some(value => typeof value !== 'string')) throw Error('command must be a nonempty array of strings');
  const targetEnv = request.env === undefined ? {} : request.env;
  if (!targetEnv || typeof targetEnv !== 'object' || Array.isArray(targetEnv) || Object.entries(targetEnv).some(([key, value]) => !key || /[=\0]/.test(key) || typeof value !== 'string' || value.includes('\0'))) throw Error('env must be an object of valid string environment overrides');
  const effectiveEnv = { ...process.env, ...targetEnv };
  const runtime = request.runtime ?? basename(command[0]).replace(/\.exe$/i, '');
  if (!['node', 'bun', 'deno'].includes(runtime)) throw Error(`unsupported JavaScript runtime: ${runtime}`);
  const directory = resolve(artifact_dir);
  mkdirSync(directory, { recursive: true });
  const probe = basename(command[0]).replace(/\.exe$/i, '') === runtime ? command[0] : runtime;
  const result = spawnSync(probe, ['--version'], { cwd, env: effectiveEnv, encoding: 'utf8', timeout: 10000 });
  if (result.status !== 0) throw Error(`cannot inspect ${runtime}: ${result.error?.message ?? result.stderr}`);
  const runtime_version = result.stdout.trim().split('\n')[0];
  const version = runtime_version.match(/(\d+)\.(\d+)/);
  const major = Number(version?.[1]), minor = Number(version?.[2]);
  if (runtime === 'node' && major < 20) throw Error('Node 20 or newer is required');
  if (runtime === 'bun' && (major < 1 || major === 1 && minor < 4)) throw Error('Bun 1.4 or newer is required for per-worker profiling');
  if (runtime === 'deno' && (major < 2 || major === 2 && minor < 8)) throw Error('Deno 2.8 or newer is required for built-in CPU profiling');
  const env = { ...targetEnv, HUNIN_RUN_ID: run_id, HUNIN_JS_PROFILE_DIR: directory };
  const diagnostics = [];
  let launched;
  if (runtime === 'bun') {
    const preload = fileURLToPath(new URL('./bun-preload.mjs', import.meta.url));
    env.BUN_OPTIONS = `${effectiveEnv.BUN_OPTIONS ?? ''} --preload=${pathToFileURL(preload).href}`.trim();
    launched = [command[0], '--preload', preload, ...command.slice(1)];
    diagnostics.push('Bun workers are sampled through Inspector preloads; worker termination waits up to 100ms for a profiling flush. Forced process termination can leave the current segment incomplete.');
  } else if (runtime === 'node') {
    const preload = fileURLToPath(new URL('./clock-preload.mjs', import.meta.url));
    env.NODE_OPTIONS = `${effectiveEnv.NODE_OPTIONS ?? ''} --cpu-prof --cpu-prof-dir=${JSON.stringify(directory)} --import=${JSON.stringify(pathToFileURL(preload).href)}`.trim();
    launched = [...command];
  } else {
    const preload = fileURLToPath(new URL('./clock-preload.mjs', import.meta.url));
    // Keep the target's explicit permissions. Hunin only adds access needed by its preload.
    const inspectorDenied = command.some(value => value === '--deny-sys' || value.startsWith('--deny-sys=') && value.slice('--deny-sys='.length).split(',').includes('inspector'));
    const index = ['run', 'eval', 'test', 'bench'].includes(command[1]) ? 2 : 1;
    launched = [...command.slice(0, index), '--cpu-prof', `--cpu-prof-dir=${directory}`, '--preload', preload,
      `--allow-env=HUNIN_JS_PROFILE_DIR,HUNIN_RUN_ID,HUNIN_RECORD_DIR`, ...(inspectorDenied ? [] : ['--allow-sys=inspector']), `--allow-write=${directory}`, ...command.slice(index)];
    if (inspectorDenied) diagnostics.push('The target denies Inspector access; CPU samples retain an unmapped runtime clock when epoch calibration is unavailable.');
    diagnostics.push('Deno TypeScript CPU profile positions refer to generated JavaScript when the runtime omits source maps. Worker profiles are emitted by Deno itself. Child Deno processes need their own collection plan.');
  }
  return { command: launched, env, profiles: [{ path: directory, suffix: '.cpuprofile', format: 'v8-cpuprofile' }], diagnostics, runtime_version };
}
function isEntrypoint() {
  try { return process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url)); }
  catch { return false; }
}
if (isEntrypoint()) {
  try { process.stdout.write(JSON.stringify(plan(JSON.parse(readFileSync(0, 'utf8')))) + '\n'); }
  catch (error) { process.stderr.write(String(error.message ?? error) + '\n'); process.exitCode = 1; }
}
