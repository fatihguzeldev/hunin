const number = new Intl.NumberFormat('en', { maximumFractionDigits: 2 });

export function numeric(value: number): string {
  return number.format(value);
}

export function percentage(value: number | null | undefined): string {
  return value == null ? '—' : `${value > 0 ? '+' : ''}${numeric(value)}%`;
}

export function duration(nanoseconds: number): string {
  if (nanoseconds < 1_000) return `${numeric(nanoseconds)} ns`;
  if (nanoseconds < 1_000_000) return `${numeric(nanoseconds / 1_000)} µs`;
  if (nanoseconds < 1_000_000_000) return `${numeric(nanoseconds / 1_000_000)} ms`;
  return `${numeric(nanoseconds / 1_000_000_000)} s`;
}

export function bytes(value: number): string {
  if (value < 1_024) return `${numeric(value)} B`;
  if (value < 1_048_576) return `${numeric(value / 1_024)} KiB`;
  if (value < 1_073_741_824) return `${numeric(value / 1_048_576)} MiB`;
  return `${numeric(value / 1_073_741_824)} GiB`;
}

export function date(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.valueOf()) ? value : parsed.toLocaleString();
}
