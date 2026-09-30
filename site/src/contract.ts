import pinned from '../../backend/openapi/v1.json';

/**
 * The API page's reference, read from the contract the backend pins and CI
 * holds to its last release. Written out by hand, the page would describe an
 * API that drifts from the one `/api/v1` serves, and nothing would notice.
 *
 * Only the slice of OpenAPI 3.1 the contract uses is typed here. The prose
 * stays as the contract writes it, in English: it is the contract.
 */
export interface Schema {
  type?: string | string[];
  $ref?: string;
  items?: Schema;
  oneOf?: Schema[];
  properties?: Record<string, Schema>;
  required?: string[];
  description?: string;
}

interface Parameter {
  name: string;
  in: string;
  required?: boolean;
  description?: string;
  schema?: Schema;
}

interface Body {
  description?: string;
  content?: Record<string, { schema?: Schema }>;
}

interface Operation {
  operationId: string;
  tags?: string[];
  summary?: string;
  description?: string;
  parameters?: Parameter[];
  requestBody?: Body;
  responses: Record<string, Body>;
  'x-routarr-scope'?: string;
}

interface Contract {
  servers: { url: string }[];
  tags: { name: string; description?: string }[];
  paths: Record<string, Record<string, Operation | Parameter[]>>;
  components: { schemas: Record<string, Schema> };
}

const contract = pinned as unknown as Contract;

const METHODS = ['get', 'put', 'post', 'delete', 'patch'];

/** Where every documented path is mounted: `/api/v1`. */
export const SERVER = contract.servers[0]!.url;

export interface Field {
  name: string;
  type: string;
  required: boolean;
  description: string;
}

export interface Answer {
  code: string;
  type: string;
  description: string;
}

export interface Endpoint {
  id: string;
  method: string;
  path: string;
  summary: string;
  description: string;
  /** `null` for the one operation a caller reaches without a key. */
  scope: string | null;
  parameters: (Field & { in: string })[];
  body: string | null;
  answers: Answer[];
}

/**
 * A type as a reader writes it, `Decision[]` or `string | null`, as HTML: a
 * schema's name links to its entry under the schemas.
 */
export function typeOf(schema: Schema | undefined): string {
  if (!schema) return 'any';
  if (schema.$ref) {
    const name = schema.$ref.split('/').pop()!;
    return `<a href="#schema-${name}">${name}</a>`;
  }
  if (schema.oneOf) return schema.oneOf.map(typeOf).join(' | ');
  const types = Array.isArray(schema.type) ? schema.type : schema.type ? [schema.type] : [];
  if (!types.length) return schema.properties ? 'object' : 'any';
  return types.map((type) => (type === 'array' ? `${typeOf(schema.items)}[]` : type)).join(' | ');
}

const escape = (text: string) =>
  text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/**
 * The contract's Markdown, as far as it goes: paragraphs, and code between
 * backticks. Escaped before anything is added, since the text is set as HTML.
 */
export function prose(text: string | undefined): string {
  return paragraphs(text).map((paragraph) => `<p>${paragraph}</p>`).join('');
}

function paragraphs(text: string | undefined): string[] {
  if (!text) return [];
  return text
    .split(/\n\s*\n/)
    .map((paragraph) => escape(paragraph.replace(/\s*\n\s*/g, ' ')).replace(/`([^`]+)`/g, '<code>$1</code>'));
}

function fieldsOf(schema: Schema): Field[] {
  const required = new Set(schema.required ?? []);
  return Object.entries(schema.properties ?? {}).map(([name, property]) => ({
    name,
    type: typeOf(property),
    required: required.has(name),
    description: prose(property.description),
  }));
}

function bodyType(body: Body | undefined): string | null {
  const [media, content] = Object.entries(body?.content ?? {})[0] ?? [];
  if (!media) return null;
  return content?.schema ? typeOf(content.schema) : escape(media);
}

function endpoint(path: string, method: string, operation: Operation, shared: Parameter[]): Endpoint {
  return {
    id: `op-${operation.operationId}`,
    method: method.toUpperCase(),
    path,
    summary: operation.summary ?? '',
    description: prose(operation.description),
    scope: operation['x-routarr-scope'] ?? null,
    parameters: [...shared, ...(operation.parameters ?? [])].map((parameter) => ({
      name: parameter.name,
      in: parameter.in,
      type: typeOf(parameter.schema),
      required: parameter.required ?? false,
      description: prose(parameter.description),
    })),
    body: bodyType(operation.requestBody),
    answers: Object.entries(operation.responses).map(([code, answer]) => ({
      code,
      type: bodyType(answer) ?? '',
      description: prose(answer.description),
    })),
  };
}

/** Every operation, under the tag that files it, in the contract's order. */
export function groups(): { name: string; description: string; endpoints: Endpoint[] }[] {
  const endpoints: (Endpoint & { tag: string })[] = [];
  for (const [path, item] of Object.entries(contract.paths)) {
    const shared = (item.parameters as Parameter[] | undefined) ?? [];
    for (const method of METHODS) {
      const operation = item[method] as Operation | undefined;
      if (!operation) continue;
      endpoints.push({ ...endpoint(path, method, operation, shared), tag: operation.tags?.[0] ?? '' });
    }
  }
  const filed = contract.tags.map((tag) => ({
    name: tag.name,
    description: prose(tag.description),
    endpoints: endpoints.filter((operation) => operation.tag === tag.name),
  }));
  /* An operation under a tag the contract does not declare still has to be on
     the page: a reference that quietly drops one is worse than an untidy one. */
  const unfiled = endpoints.filter((operation) => !contract.tags.some((tag) => tag.name === operation.tag));
  if (unfiled.length) filed.push({ name: 'other', description: '', endpoints: unfiled });
  return filed;
}

/**
 * Every schema the operations name, in the contract's order. The first
 * paragraph of its description is `note`, which sits in a `<summary>` and so
 * takes no `<p>`, and `more` holds the rest.
 */
export function schemas(): { name: string; type: string; note: string; more: string; fields: Field[] }[] {
  return Object.entries(contract.components.schemas).map(([name, schema]) => {
    const [note = '', ...more] = paragraphs(schema.description);
    return {
      name,
      type: typeOf(schema),
      note,
      more: more.map((paragraph) => `<p>${paragraph}</p>`).join(''),
      fields: fieldsOf(schema),
    };
  });
}
