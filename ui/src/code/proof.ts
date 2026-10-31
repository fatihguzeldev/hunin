/** Format exact retained bytes; the left column stays in artifact byte coordinates. */
export function hexBytes(content: string, offset: number): string {
  if (content.length % 2 || !/^[0-9a-f]*$/i.test(content))
    throw new Error(
      'The artifact service returned an invalid hexadecimal excerpt.',
    );
  const lines: string[] = [];
  for (let start = 0; start < content.length; start += 32)
    lines.push(
      `0x${(offset + start / 2).toString(16)}  ${content
        .slice(start, start + 32)
        .match(/../g)!
        .join(' ')}`,
    );
  return lines.join('\n');
}

/** Keep file names recognizable; exact names and identities belong in proof details. */
export function proofName(name: string | undefined, index: number): string {
  if (!name) return `Proof ${index + 1}`;
  const brief = name.replace(/[a-f0-9]{24,}/gi, '…');
  return brief.length > 48
    ? `${brief.slice(0, 27)}…${brief.slice(-18)}`
    : brief;
}
