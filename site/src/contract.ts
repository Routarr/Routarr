/**
 * What the API can do, read from the contract of the release the page names
 * (`release.mjs`) and shaped for the reference list: each operation with what
 * it takes and the top level of what it answers. Field names and types are
 * the contract's own, which no language translates. What an operation does and
 * what a group holds come from the catalogues, keyed by the contract's
 * `operationId` and tag, which `check.mjs` holds to `backend/openapi/v1.json`.
 */
import local from '../../backend/openapi/v1.json';
import { releasedContract } from '../release.mjs';
import { version } from './version';

interface Schema {
  $ref?: string;
  type?: string | string[];
  items?: Schema;
  properties?: Record<string, Schema>;
  required?: string[];
  allOf?: Schema[];
  oneOf?: Schema[];
  anyOf?: Schema[];
}

interface Parameter {
  name: string;
  in: 'path' | 'query' | 'header';
  required?: boolean;
  schema?: Schema;
}

interface RawOperation {
  operationId: string;
  tags?: string[];
  'x-routarr-scope'?: string;
  parameters?: Parameter[];
  requestBody?: { content?: Record<string, { schema?: Schema }> };
  responses?: Record<string, { content?: Record<string, { schema?: Schema }> }>;
}

interface Contract {
  tags?: { name: string }[];
  paths: Record<string, Record<string, RawOperation>>;
  components?: { schemas?: Record<string, Schema> };
}

export interface Field {
  name: string;
  type: string;
  required: boolean;
  /** Where a parameter travels. Absent for a field of a body or an answer. */
  where?: 'path' | 'query' | 'header';
}

export interface Operation {
  id: string;
  method: string;
  path: string;
  /** The scope a key needs, or `null` for the one operation that needs no key. */
  scope: string | null;
  params: Field[];
  body: Field[];
  answer: { status: string; type: string; fields: Field[] } | null;
}

export interface Group {
  tag: string;
  operations: Operation[];
}

const contract = (await releasedContract(version, local)) as Contract;
const schemas = contract.components?.schemas ?? {};

const named = (ref: string) => ref.split('/').pop() ?? ref;
const resolve = (schema: Schema): Schema =>
  schema.$ref ? resolve(schemas[named(schema.$ref)] ?? {}) : schema;

/** A type as the contract names it: a schema's name, a list of one, or a primitive. */
function typeOf(schema: Schema | undefined): string {
  if (!schema) return 'object';
  if (schema.$ref) return named(schema.$ref);
  const union = schema.oneOf ?? schema.anyOf ?? schema.allOf;
  if (union) return union.map(typeOf).filter((t) => t !== 'null').join(' | ');
  const types = Array.isArray(schema.type) ? schema.type : [schema.type ?? 'object'];
  const kept = types.filter((t) => t !== 'null');
  const base = kept[0] === 'array' ? `${typeOf(schema.items)}[]` : (kept[0] ?? 'object');
  return types.includes('null') ? `${base}?` : base;
}

/** The top-level fields of an object schema, its parts merged. */
function fieldsOf(schema: Schema | undefined): Field[] {
  if (!schema) return [];
  const object = resolve(schema);
  const parts = object.allOf ? object.allOf.map(resolve) : [object];
  return parts.flatMap((part) =>
    Object.entries(part.properties ?? {}).map(([name, property]) => ({
      name,
      type: typeOf(property),
      required: (part.required ?? []).includes(name),
    })),
  );
}

function json(content: Record<string, { schema?: Schema }> | undefined): Schema | undefined {
  return content?.['application/json']?.schema;
}

function answerOf(operation: RawOperation): Operation['answer'] {
  const status = Object.keys(operation.responses ?? {}).find((code) => code.startsWith('2'));
  if (!status) return null;
  const schema = json(operation.responses?.[status]?.content);
  if (!schema) return { status, type: '', fields: [] };
  const top = resolve(schema);
  const listed = top.type === 'array' || (Array.isArray(top.type) && top.type.includes('array'));
  return {
    status,
    type: typeOf(schema),
    fields: fieldsOf(listed ? top.items : schema),
  };
}

const operations: (Operation & { tag: string })[] = Object.entries(contract.paths).flatMap(
  ([path, methods]) =>
    Object.entries(methods).map(([method, operation]) => ({
      id: operation.operationId,
      tag: operation.tags?.[0] ?? '',
      method: method.toUpperCase(),
      path,
      scope: operation['x-routarr-scope'] ?? null,
      params: (operation.parameters ?? []).map((parameter) => ({
        name: parameter.name,
        type: typeOf(parameter.schema),
        required: parameter.required ?? false,
        where: parameter.in,
      })),
      body: fieldsOf(json(operation.requestBody?.content)),
      answer: answerOf(operation),
    })),
);

/** The groups in the contract's own order, each with its operations. */
export const groups: Group[] = (contract.tags ?? [])
  .map(({ name }) => ({ tag: name, operations: operations.filter((op) => op.tag === name) }))
  .filter((group) => group.operations.length > 0);
