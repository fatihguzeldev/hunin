export type Attributes = Record<string, unknown>;
export interface Context { run_id: string; context_id?: string; parent_id?: string; }
export interface Span { readonly id: string | undefined; end(attributes?: Attributes): void; run<T>(callback: () => T): T; }
export function enabled(): boolean;
export function lastError(): unknown;
export function flush(): void;
export function captureContext(): Context | undefined;
export function context(name: string, attributes?: Attributes): string | undefined;
export function withContext<T>(value: string | Context | undefined, callback: () => T, attributes?: Attributes): T;
export function startSpan(name: string, attributes?: Attributes): Span;
export function withSpan<T>(name: string, callback: (span?: Span) => T, attributes?: Attributes): T;
export function counter(name: string, value: number, unit?: string, attributes?: Attributes): string | undefined;
export function event(name: string, attributes?: Attributes): string | undefined;
export function relation(from: string | undefined, to: string | undefined, name?: string, evidence?: string[]): string | undefined;
