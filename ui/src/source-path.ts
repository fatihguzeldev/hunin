/** Virtual runtime locations remain visible without pretending to be local files. */
export function hasLocalSource(path?: string | null): path is string {
  if (!path) return false;
  if (/^[a-z]:[\\/]/i.test(path)) return true;
  return !/^[a-z][a-z0-9+.-]*:/i.test(path) || path.startsWith('file:');
}
