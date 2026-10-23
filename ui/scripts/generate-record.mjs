import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

export function typeOf(schema) {
  if ('const' in schema) return JSON.stringify(schema.const);
  if (schema.enum) return schema.enum.map(value => JSON.stringify(value)).join(' | ');
  if (schema.type === 'array') return `Array<${typeOf(schema.items)}>`;
  if (schema.type === 'object') {
    if (!schema.properties) return 'Record<string, unknown>';
    return `{\n${Object.entries(schema.properties).map(([key, value]) =>
      `  ${JSON.stringify(key)}${schema.required?.includes(key) ? '' : '?'}: ${typeOf(value)};`).join('\n')}\n}`;
  }
  return { string: 'string', number: 'number', integer: 'number', boolean: 'boolean', null: 'null' }[schema.type] ?? 'unknown';
}

export function generate(schema) {
  const variants = schema.allOf.map(condition => {
    const kind = condition.if.properties.kind.const;
    return `{ kind: ${JSON.stringify(kind)} } & Required<Pick<RecordBase, ${condition.then.required.map(key => JSON.stringify(key)).join(' | ')}>>`;
  });
  return `// Generated from protocol/record.schema.json. Run npm run generate; do not edit.\n` +
    `type RecordBase = ${typeOf(schema)};\n\n` +
    `export type Observation = RecordBase & (\n  ${variants.join('\n  | ')}\n);\n`;
}

export function generateOperations(operations) {
  return '// Generated from protocol/operations.json. Run npm run generate; do not edit.\n' +
    'export type OperationInputs = {\n' + operations.map(operation => `  ${JSON.stringify(operation.name)}: ${typeOf(operation.inputSchema)};`).join('\n') + '\n};\n';
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const schema = JSON.parse(await readFile(new URL('../../protocol/record.schema.json', import.meta.url), 'utf8'));
  const operations = JSON.parse(await readFile(new URL('../../protocol/operations.json', import.meta.url), 'utf8'));
  for (const [name, generated] of [['record', generate(schema)], ['operations', generateOperations(operations)]]) {
    const target = new URL(`../src/${name}.generated.ts`, import.meta.url);
    if (process.argv.includes('--check')) {
      if (await readFile(target, 'utf8') !== generated) throw new Error(`${name} types are stale. Run npm run generate.`);
    } else await writeFile(target, generated);
  }
}
