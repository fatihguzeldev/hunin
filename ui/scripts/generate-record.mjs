import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

export function typeOf(schema) {
  if ('const' in schema) return JSON.stringify(schema.const);
  if (schema.enum) return schema.enum.map(value => JSON.stringify(value)).join(' | ');
  if (Array.isArray(schema.type)) return schema.type.map(type => typeOf({ ...schema, type })).join(' | ');
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
  const outputs = operations.filter(operation => operation.outputSchema);
  return '// Generated from protocol/operations.json. Run npm run generate; do not edit.\n' +
    'export type OperationInputs = {\n' + operations.map(operation => `  ${JSON.stringify(operation.name)}: ${typeOf(operation.inputSchema)};`).join('\n') + '\n};\n'
    + '\nexport type OperationOutputs = {\n' + outputs.map(operation => `  ${JSON.stringify(operation.name)}: ${typeOf(operation.outputSchema)};`).join('\n') + '\n};\n'
    + '\nfunction isOutputObject(value: unknown): value is Record<string, unknown> {\n  return value !== null && typeof value === "object" && !Array.isArray(value);\n}\n'
    + '\n// Only declared output schemas are checked while the catalog is migrated.\n'
    + 'export function validateOperationOutput(name: string, value: unknown): void {\n  switch (name) {\n'
    + outputs.map(operation => `    case ${JSON.stringify(operation.name)}:\n      if (!${outputCondition(operation.outputSchema, 'value')}) throw new Error(${JSON.stringify(`Hunin returned an invalid ${operation.name} result.`)});\n      break;`).join('\n')
    + '\n  }\n}\n';
}

// Generate checks for the output vocabulary we actually publish. Reject new
// unsupported schema keywords rather than silently generating a weaker check.
function outputCondition(schema, value) {
  const supported = new Set(['type', 'const', 'enum', 'properties', 'required', 'additionalProperties', 'items', 'minimum', 'maximum', 'description']);
  for (const key of Object.keys(schema)) {
    if (!supported.has(key)) throw new Error(`Unsupported operation output schema keyword: ${key}`);
  }
  if (Array.isArray(schema.type)) return `(${schema.type.map(type => outputCondition({ ...schema, type }, value)).join(' || ')})`;
  const checks = [];
  if ('const' in schema) checks.push(`${value} === ${JSON.stringify(schema.const)}`);
  if (schema.enum) checks.push(`(${schema.enum.map(item => `${value} === ${JSON.stringify(item)}`).join(' || ')})`);
  switch (schema.type) {
    case undefined: break;
    case 'null': checks.push(`${value} === null`); break;
    case 'string': case 'boolean': checks.push(`typeof ${value} === ${JSON.stringify(schema.type)}`); break;
    case 'integer': case 'number':
      checks.push(`typeof ${value} === "number"`, `Number.${schema.type === 'integer' ? 'isInteger' : 'isFinite'}(${value})`);
      if (schema.minimum !== undefined) checks.push(`${value} >= ${schema.minimum}`);
      if (schema.maximum !== undefined) checks.push(`${value} <= ${schema.maximum}`);
      break;
    case 'array':
      checks.push(`Array.isArray(${value})`);
      if (schema.items) checks.push(`${value}.every(item => ${outputCondition(schema.items, 'item')})`);
      break;
    case 'object': {
      checks.push(`isOutputObject(${value})`);
      for (const key of schema.required ?? []) checks.push(`Object.hasOwn(${value}, ${JSON.stringify(key)})`);
      for (const [key, property] of Object.entries(schema.properties ?? {})) {
        const condition = outputCondition(property, `${value}[${JSON.stringify(key)}]`);
        checks.push(schema.required?.includes(key) ? condition : `(!Object.hasOwn(${value}, ${JSON.stringify(key)}) || ${condition})`);
      }
      if (schema.additionalProperties === false) {
        checks.push(`Object.keys(${value}).every(key => ${JSON.stringify(Object.keys(schema.properties ?? {}))}.includes(key))`);
      } else if (schema.additionalProperties && typeof schema.additionalProperties === 'object') {
        checks.push(`Object.entries(${value}).every(([key, item]) => ${JSON.stringify(Object.keys(schema.properties ?? {}))}.includes(key) || ${outputCondition(schema.additionalProperties, 'item')})`);
      }
      break;
    }
    default: throw new Error(`Unsupported operation output schema type: ${schema.type}`);
  }
  return checks.length ? `(${checks.join(' && ')})` : 'true';
}

// Git may check out generated sources with CRLF on Windows. Ignore only that
// representation difference; schema or generated type changes must still fail.
export function generatedMatches(actual, expected) {
  return actual.replace(/\r\n/g, '\n') === expected;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const schema = JSON.parse(await readFile(new URL('../../protocol/record.schema.json', import.meta.url), 'utf8'));
  const operations = JSON.parse(await readFile(new URL('../../protocol/operations.json', import.meta.url), 'utf8'));
  for (const [name, generated] of [['record', generate(schema)], ['operations', generateOperations(operations)]]) {
    const target = new URL(`../src/${name}.generated.ts`, import.meta.url);
    if (process.argv.includes('--check')) {
      if (!generatedMatches(await readFile(target, 'utf8'), generated)) throw new Error(`${name} types are stale. Run npm run generate.`);
    } else await writeFile(target, generated);
  }
}
