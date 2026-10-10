/**
 * What the API can do, read from this checkout's contract (production builds
 * at the release's tag) and shaped for the reference list: each operation with
 * what it takes and the top level of what it answers, then every type those
 * name. Field names and types are the contract's own, which no language
 * translates. What an operation does and what a group holds come from the
 * catalogues, keyed by the contract's `operationId` and tag, which `check.mjs`
 * holds to `backend/openapi/v1.json`.
 */
import raw from '../../backend/openapi/v1.json';

interface Schema {
  $ref?: string;
  type?: string | string[];
  enum?: string[];
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
  servers?: { url: string }[];
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
  /** The full path a client calls, the contract's server prefix included. */
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

/** A named type, in the one shape its schema has. */
export type Type =
  | { name: string; kind: 'fields'; fields: Field[] }
  | { name: string; kind: 'values'; values: string[] }
  | { name: string; kind: 'variants'; by: string; variants: { tag: string; fields: Field[] }[] }
  | { name: string; kind: 'list'; of: string };

const contract = raw as Contract;
const schemas = contract.components?.schemas ?? {};
const server = contract.servers?.[0]?.url ?? '';

const named = (ref: string) => ref.split('/').pop() ?? ref;
const resolve = (schema: Schema): Schema =>
  schema.$ref ? resolve(schemas[named(schema.$ref)] ?? {}) : schema;
const isNull = (schema: Schema) => schema.type === 'null';

/**
 * A type as the contract names it: a schema's name, a list of one, a
 * primitive, `?` when it may be null and `&` for a composition.
 */
function typeOf(schema: Schema | undefined): string {
  if (!schema) return 'object';
  if (schema.$ref) return named(schema.$ref);
  if (schema.allOf) return schema.allOf.map(typeOf).join(' & ');
  const union = schema.oneOf ?? schema.anyOf;
  if (union) {
    const kept = union.filter((member) => !isNull(member));
    const typed = kept.map(typeOf).join(' | ');
    if (kept.length === union.length) return typed;
    return kept.length > 1 ? `(${typed})?` : `${typed}?`;
  }
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

const success = (operation: RawOperation) =>
  Object.keys(operation.responses ?? {}).find((code) => code.startsWith('2'));

function answerOf(operation: RawOperation): Operation['answer'] {
  const status = success(operation);
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

const raws = Object.entries(contract.paths).flatMap(([path, methods]) =>
  Object.entries(methods).map(([method, operation]) => ({ path, method, operation })),
);

const operations: (Operation & { tag: string })[] = raws.map(({ path, method, operation }) => ({
  id: operation.operationId,
  tag: operation.tags?.[0] ?? '',
  method: method.toUpperCase(),
  path: `${server}${path}`,
  scope: operation['x-routarr-scope'] ?? null,
  params: (operation.parameters ?? []).map((parameter) => ({
    name: parameter.name,
    type: typeOf(parameter.schema),
    required: parameter.required ?? false,
    where: parameter.in,
  })),
  body: fieldsOf(json(operation.requestBody?.content)),
  answer: answerOf(operation),
}));

/** The groups in the contract's own order, each with its operations. */
export const groups: Group[] = (contract.tags ?? [])
  .map(({ name: tag }) => ({ tag, operations: operations.filter((op) => op.tag === tag) }))
  .filter((group) => group.operations.length > 0);

/** Every schema an operation reaches, through the fields of another included. */
function reached(): Set<string> {
  const found = new Set<string>();
  const visit = (schema: Schema | undefined): void => {
    if (!schema) return;
    if (schema.$ref) {
      const name = named(schema.$ref);
      if (found.has(name)) return;
      found.add(name);
      visit(schemas[name]);
      return;
    }
    for (const part of [...(schema.allOf ?? []), ...(schema.oneOf ?? []), ...(schema.anyOf ?? [])]) visit(part);
    for (const property of Object.values(schema.properties ?? {})) visit(property);
    visit(schema.items);
  };
  for (const { operation } of raws) {
    for (const parameter of operation.parameters ?? []) visit(parameter.schema);
    visit(json(operation.requestBody?.content));
    const status = success(operation);
    if (status) visit(json(operation.responses?.[status]?.content));
  }
  return found;
}

/** The property whose single allowed value names each variant of a tagged union. */
function tagOf(variant: Schema): string | undefined {
  return Object.entries(variant.properties ?? {}).find(([, property]) => property.enum?.length === 1)?.[0];
}

function shapeOf(name: string, schema: Schema): Type {
  if (schema.enum) return { name, kind: 'values', values: schema.enum };
  if (schema.type === 'array') return { name, kind: 'list', of: typeOf(schema.items) };
  const variants = schema.oneOf ?? schema.anyOf;
  const by = variants?.[0] && tagOf(variants[0]);
  if (variants && by) {
    return {
      name,
      kind: 'variants',
      by,
      variants: variants.map((variant) => ({
        tag: variant.properties?.[by]?.enum?.[0] ?? '',
        fields: fieldsOf(variant).filter((field) => field.name !== by),
      })),
    };
  }
  return { name, kind: 'fields', fields: fieldsOf(schema) };
}

/** Every type the operations name, by name. */
export const types: Type[] = [...reached()]
  .sort((a, b) => a.localeCompare(b))
  .map((name) => shapeOf(name, schemas[name] ?? {}));

const typeNames = new Set(types.map((type) => type.name));

/**
 * A type written as text and links: each name a type of the list above is
 * one of its own, the punctuation (`[]`, `?`, ` | `) text around it.
 */
export function typeParts(type: string): { text: string; type?: string }[] {
  return type
    .split(/([A-Za-z_][A-Za-z0-9_]*)/)
    .filter((text) => text !== '')
    .map((text) => (typeNames.has(text) ? { text, type: text } : { text }));
}
