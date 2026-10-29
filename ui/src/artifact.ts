const profileFormats = new Set([
  'firefox',
  'firefox-profile',
  'samply',
  'v8',
  'v8-cpuprofile',
  'cpuprofile',
  'graal',
  'graal-cpusampler',
  'pprof',
  'go-pprof',
  'go-trace',
  'vmprof',
  'python-cpu-progress',
]);

export function canReanalyze(format: string): boolean {
  return profileFormats.has(format);
}
