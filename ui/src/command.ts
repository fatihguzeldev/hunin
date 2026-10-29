/** Argument arrays preserve exact boundaries without an implicit shell. */
export function parseCommand(value: string): string[] {
  const parsed: unknown = JSON.parse(value);
  if (
    !Array.isArray(parsed) ||
    !parsed.length ||
    parsed.some((item) => typeof item !== 'string') ||
    !parsed[0]
  ) {
    throw new Error('Command must be a non-empty JSON array of strings.');
  }
  return parsed as string[];
}
