export {};

declare global {
  var Deno: { version: { deno: string } } | undefined;
  var Bun: { version: string } | undefined;
}
