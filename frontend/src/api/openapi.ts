/**
 * Reading the API contract, `/api/v1/openapi.json`, for the reference screen.
 *
 * The document is generated from the backend's own types, so the screen reads
 * it rather than restating it: a route added to the contract appears here
 * with no change to the interface. Types are written in a notation every
 * language reads the same way (`Decision[]`, `string | null`).
 */

export interface OpenApiSchema {
  $ref?: string;
  type?: string | string[];
  items?: OpenApiSchema;
  properties?: Record<string, OpenApiSchema>;
  required?: string[];
  oneOf?: OpenApiSchema[];
  anyOf?: OpenApiSchema[];
  allOf?: OpenApiSchema[];
  description?: string;
  additionalProperties?: OpenApiSchema | boolean;
}

export interface OpenApiParameter {
  name: string;
  in: string;
  required?: boolean;
  description?: string;
  schema?: OpenApiSchema;
}

interface OpenApiMedia {
  schema?: OpenApiSchema;
}

export interface OpenApiResponse {
  description?: string;
  content?: Record<string, OpenApiMedia>;
}

export interface OpenApiOperation {
  operationId?: string;
  summary?: string;
  description?: string;
  tags?: string[];
  parameters?: OpenApiParameter[];
  requestBody?: { content?: Record<string, OpenApiMedia> };
  responses?: Record<string, OpenApiResponse>;
  security?: unknown[];
  'x-routarr-scope'?: string;
}

export interface OpenApiDocument {
  info: { title: string; version: string; description?: string };
  servers?: { url: string }[];
  paths: Record<string, Record<string, OpenApiOperation>>;
  tags?: { name: string; description?: string }[];
  components?: { schemas?: Record<string, OpenApiSchema> };
}

/** One operation, flattened for the screen. */
export interface Operation {
  id: string;
  method: string;
  path: string;
  tag: string;
  summary: string;
  description: string;
  /** `read`, `operate` or `write`, `null` for the one operation needing no key. */
  scope: string | null;
  parameters: OpenApiParameter[];
  body: OpenApiSchema | null;
  responses: { code: string; description: string; schema: OpenApiSchema | null }[];
}

export interface Group {
  tag: string;
  description: string;
  operations: Operation[];
}

export interface Field {
  name: string;
  type: string;
  required: boolean;
  description: string;
}

const METHODS = ['get', 'post', 'put', 'patch', 'delete'];

function jsonSchema(content: Record<string, OpenApiMedia> | undefined): OpenApiSchema | null {
  return content?.['application/json']?.schema ?? null;
}

/** Every operation, in the document's path order. */
export function operations(doc: OpenApiDocument): Operation[] {
  return Object.entries(doc.paths).flatMap(([path, item]) =>
    METHODS.filter((method) => item[method]).map((method) => {
      const operation = item[method] as OpenApiOperation;
      return {
        id: operation.operationId ?? `${method}-${path}`,
        method,
        path,
        tag: operation.tags?.[0] ?? '',
        summary: operation.summary ?? '',
        description: operation.description ?? '',
        scope: operation['x-routarr-scope'] ?? null,
        parameters: operation.parameters ?? [],
        body: jsonSchema(operation.requestBody?.content),
        responses: Object.entries(operation.responses ?? {}).map(([code, response]) => ({
          code,
          description: response.description ?? '',
          schema: jsonSchema(response.content),
        })),
      };
    }),
  );
}

/** The operations grouped by tag, in the order the document lists its tags. */
export function byTag(doc: OpenApiDocument): Group[] {
  const all = operations(doc);
  const declared = (doc.tags ?? []).map((tag) => tag.name);
  const others = [...new Set(all.map((op) => op.tag))].filter((tag) => !declared.includes(tag));
  return [...declared, ...others]
    .map((tag) => ({
      tag,
      description: doc.tags?.find((entry) => entry.name === tag)?.description ?? '',
      operations: all.filter((op) => op.tag === tag),
    }))
    .filter((group) => group.operations.length > 0);
}

/** `Decision` for `#/components/schemas/Decision`. */
export function refName(schema: OpenApiSchema): string | null {
  return schema.$ref?.split('/').pop() ?? null;
}

/** The schema a reference points at, the schema itself otherwise. */
export function resolve(schema: OpenApiSchema, doc: OpenApiDocument): OpenApiSchema {
  const name = refName(schema);
  return name ? (doc.components?.schemas?.[name] ?? {}) : schema;
}

function isNull(schema: OpenApiSchema): boolean {
  return schema.type === 'null';
}

/** A type as the reference writes it: `Decision[]`, `string | null`, `object`. */
export function typeLabel(schema: OpenApiSchema | null | undefined): string {
  if (!schema) return 'object';
  const name = refName(schema);
  if (name) return name;
  const variants = schema.oneOf ?? schema.anyOf;
  if (variants) return variants.map(typeLabel).join(' | ');
  if (schema.allOf?.length === 1 && schema.allOf[0]) return typeLabel(schema.allOf[0]);
  if (Array.isArray(schema.type)) {
    return schema.type
      .map((type) => (type === 'array' ? `${typeLabel(schema.items)}[]` : type))
      .join(' | ');
  }
  if (schema.type === 'array') return `${typeLabel(schema.items)}[]`;
  return schema.type ?? 'object';
}

/** The fields of an object schema, a reference followed and `allOf` merged. */
export function fields(schema: OpenApiSchema | null, doc: OpenApiDocument): Field[] {
  if (!schema) return [];
  const resolved = resolve(schema, doc);
  const parts = resolved.allOf ? resolved.allOf.map((part) => resolve(part, doc)) : [resolved];
  return parts.flatMap((part) =>
    Object.entries(part.properties ?? {}).map(([name, property]) => ({
      name,
      type: typeLabel(property),
      required: (part.required ?? []).includes(name),
      description:
        property.description ?? property.oneOf?.find((v) => !isNull(v))?.description ?? '',
    })),
  );
}

/** A body to start from: each required field, or every field when none is. */
function example(schema: OpenApiSchema, doc: OpenApiDocument, depth: number): unknown {
  const resolved = resolve(schema, doc);
  const type = Array.isArray(resolved.type)
    ? resolved.type.find((t) => t !== 'null')
    : resolved.type;
  if (resolved.oneOf) {
    const concrete = resolved.oneOf.find((variant) => !isNull(variant));
    return concrete ? example(concrete, doc, depth) : null;
  }
  if (type === 'array') return [];
  if (type === 'string') return '';
  if (type === 'integer' || type === 'number') return 0;
  if (type === 'boolean') return false;
  if (depth > 1) return {};
  const all = fields(resolved, doc);
  const chosen = all.some((field) => field.required) ? all.filter((f) => f.required) : all;
  const properties = { ...resolved.properties };
  return Object.fromEntries(
    chosen.map((field) => [field.name, example(properties[field.name] ?? {}, doc, depth + 1)]),
  );
}

/** The document's server, made absolute against the page's origin. */
export function serverUrl(doc: OpenApiDocument, origin: string): string {
  return `${origin}${doc.servers?.[0]?.url ?? '/api/v1'}`;
}

/** A `curl` line that calls the operation, the key read from `$ROUTARR_KEY`. */
export function curl(op: Operation, doc: OpenApiDocument, server: string): string {
  const query = op.parameters
    .filter((param) => param.in === 'query' && param.required)
    .map((param) => `${param.name}=<${param.name}>`)
    .join('&');
  const url = `${server}${op.path}${query ? `?${query}` : ''}`;
  const lines = [`curl${op.method === 'get' ? '' : ` -X ${op.method.toUpperCase()}`} "${url}"`];
  if (op.scope) lines.push(`-H "X-Api-Key: $ROUTARR_KEY"`);
  if (op.body) {
    lines.push('-H "Content-Type: application/json"');
    lines.push(`-d '${JSON.stringify(example(op.body, doc, 0))}'`);
  }
  return lines.join(' \\\n  ');
}

/** Prose in paragraphs: a blank line parts them, a single break does not. */
export function paragraphs(text: string): string[] {
  return text
    .split(/\n\s*\n/)
    .map((paragraph) => paragraph.replace(/\s*\n\s*/g, ' ').trim())
    .filter(Boolean);
}

/** A paragraph cut where its backticks mark code. */
export function inline(text: string): { text: string; code: boolean }[] {
  return text
    .split('`')
    .map((part, index) => ({ text: part, code: index % 2 === 1 }))
    .filter((part) => part.text !== '');
}
