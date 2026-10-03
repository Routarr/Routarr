import { describe, it, expect } from 'vitest';

import {
  byTag,
  curl,
  fields,
  inline,
  operations,
  paragraphs,
  serverUrl,
  typeLabel,
  type OpenApiDocument,
} from './openapi';

/**
 * The reference screen shows what the backend's contract says, so these read
 * a small document shaped like the one `/api/v1/openapi.json` serves.
 */
const DOC: OpenApiDocument = {
  info: { title: 'Routarr API', version: '1', description: 'One.\n\nTwo `code` here.' },
  servers: [{ url: '/routarr/api/v1' }],
  tags: [
    { name: 'status', description: 'Whether Routarr is up.' },
    { name: 'proposals', description: 'Simulating and applying.' },
  ],
  paths: {
    '/decisions/apply': {
      post: {
        operationId: 'apply',
        summary: 'Apply chosen proposals',
        tags: ['proposals'],
        'x-routarr-scope': 'operate',
        requestBody: {
          content: {
            'application/json': { schema: { $ref: '#/components/schemas/ApplyRequest' } },
          },
        },
        responses: {
          '200': {
            content: { 'application/json': { schema: { $ref: '#/components/schemas/Report' } } },
          },
          default: { description: 'A refusal.' },
        },
      },
    },
    '/ping': {
      get: {
        operationId: 'ping',
        summary: 'Whether Routarr is up',
        tags: ['status'],
        security: [],
        responses: {},
      },
    },
    '/route': {
      get: {
        operationId: 'place',
        tags: ['library'],
        'x-routarr-scope': 'read',
        parameters: [
          { name: 'type', in: 'query', required: true },
          { name: 'tags', in: 'query', required: false },
        ],
        responses: {},
      },
    },
    '/overrides/by-external-id/{source}/{id}': {
      put: {
        operationId: 'pin',
        tags: ['library'],
        'x-routarr-scope': 'write',
        responses: {},
      },
      delete: {
        operationId: 'unpin',
        tags: ['library'],
        'x-routarr-scope': 'write',
        responses: {},
      },
    },
  },
  components: {
    schemas: {
      ApplyRequest: {
        type: 'object',
        required: ['decision_ids'],
        properties: {
          decision_ids: { type: 'array', items: { type: 'string' } },
          move_files: { type: 'boolean', description: 'Move the files too.' },
        },
      },
      Report: {
        allOf: [
          { $ref: '#/components/schemas/Counts' },
          {
            type: 'object',
            required: ['errors'],
            properties: { errors: { type: 'array', items: { $ref: '#/components/schemas/E' } } },
          },
        ],
      },
      Counts: {
        type: 'object',
        required: ['applied'],
        properties: {
          applied: { type: 'integer' },
          metadata: {
            oneOf: [
              { $ref: '#/components/schemas/M', description: 'Null until known.' },
              { type: 'null' },
            ],
          },
          subject: { type: ['string', 'null'] },
        },
      },
    },
  },
};

describe('the API reference', () => {
  it('groups operations by the tags the document declares, then the others', () => {
    const groups = byTag(DOC);
    expect(groups.map((group) => group.tag)).toEqual(['status', 'proposals', 'library']);
    expect(groups[0]?.description).toBe('Whether Routarr is up.');
    expect(groups[1]?.operations.map((op) => op.id)).toEqual(['apply']);
  });

  it('reads the scope, and none for the operation needing no key', () => {
    const scopes = Object.fromEntries(operations(DOC).map((op) => [op.id, op.scope]));
    expect(scopes).toEqual({
      apply: 'operate',
      ping: null,
      place: 'read',
      pin: 'write',
      unpin: 'write',
    });
  });

  it('keeps every method a path carries', () => {
    const pinning = operations(DOC).filter((op) => op.path.startsWith('/overrides/'));
    expect(pinning.map((op) => [op.method, op.id])).toEqual([
      ['put', 'pin'],
      ['delete', 'unpin'],
    ]);
  });

  it('writes types in a notation no language has to translate', () => {
    expect(typeLabel({ $ref: '#/components/schemas/Decision' })).toBe('Decision');
    expect(typeLabel({ type: 'array', items: { $ref: '#/components/schemas/Job' } })).toBe('Job[]');
    expect(typeLabel({ type: ['string', 'null'] })).toBe('string | null');
    expect(typeLabel({ oneOf: [{ $ref: '#/components/schemas/M' }, { type: 'null' }] })).toBe(
      'M | null',
    );
    expect(typeLabel(undefined)).toBe('object');
  });

  it('lists the fields of a schema, following references and merging allOf', () => {
    const report = fields({ $ref: '#/components/schemas/Report' }, DOC);
    expect(report).toEqual([
      { name: 'applied', type: 'integer', required: true, description: '' },
      { name: 'metadata', type: 'M | null', required: false, description: 'Null until known.' },
      { name: 'subject', type: 'string | null', required: false, description: '' },
      { name: 'errors', type: 'E[]', required: true, description: '' },
    ]);
  });

  it('builds a curl line with the key, the required query and a body to start from', () => {
    const server = serverUrl(DOC, 'https://routarr.example');
    expect(server).toBe('https://routarr.example/routarr/api/v1');
    const byId = Object.fromEntries(operations(DOC).map((op) => [op.id, op]));

    expect(curl(byId.apply!, DOC, server)).toBe(
      [
        'curl -X POST "https://routarr.example/routarr/api/v1/decisions/apply"',
        '-H "X-Api-Key: $ROUTARR_KEY"',
        '-H "Content-Type: application/json"',
        `-d '{"decision_ids":[]}'`,
      ].join(' \\\n  '),
    );
    expect(curl(byId.ping!, DOC, server)).toBe(
      'curl "https://routarr.example/routarr/api/v1/ping"',
    );
    expect(curl(byId.place!, DOC, server)).toContain('/route?type=<type>"');
  });

  it('cuts prose into paragraphs and marks its code', () => {
    expect(paragraphs('One line\nstill one.\n\nTwo.')).toEqual(['One line still one.', 'Two.']);
    expect(inline('Send `confirm` back.')).toEqual([
      { text: 'Send ', code: false },
      { text: 'confirm', code: true },
      { text: ' back.', code: false },
    ]);
  });
});
