import { describe, it, expect, vi, afterEach } from 'vitest';
import { screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import type { OpenApiDocument } from '../api/openapi';
import ApiReference from './ApiReference.svelte';

/**
 * The reference reads the contract the running version serves, groups it by
 * what an application wants to do, and says which scope each operation asks.
 */

const STRINGS = {
  ApiReference: 'API reference',
  ApiTagStatus: 'Status',
  ApiTagProposals: 'Proposals',
  ApiNoKey: 'no key',
  ScopeOperate: 'operate',
  ApiSearch: 'Search the operations',
  ApiNoMatch: 'No operation matches.',
  ApiParameters: 'Parameters',
  ApiExample: 'Example',
  Name: 'Name',
  Yes: 'Yes',
  No: 'No',
};

const DOC: OpenApiDocument = {
  info: { title: 'Routarr API', version: '1', description: 'Send `X-Api-Key`.' },
  servers: [{ url: '/api/v1' }],
  tags: [
    { name: 'status', description: 'Whether Routarr is up.' },
    { name: 'proposals', description: 'Applying.' },
  ],
  paths: {
    '/ping': { get: { operationId: 'ping', summary: 'Whether Routarr is up', tags: ['status'] } },
    '/decisions/apply': {
      post: {
        operationId: 'apply',
        summary: 'Apply chosen proposals',
        tags: ['proposals'],
        'x-routarr-scope': 'operate',
        parameters: [{ name: 'Prefer', in: 'header', required: false }],
      },
    },
  },
  components: { schemas: { Pong: { type: 'object', properties: { status: { type: 'string' } } } } },
};

function show() {
  vi.spyOn(api, 'openApi').mockResolvedValue(DOC);
  return renderWithI18n(ApiReference, { strings: STRINGS });
}

afterEach(() => vi.restoreAllMocks());

describe('ApiReference', () => {
  it('groups the operations and names the scope each asks, or none', async () => {
    show();

    const status = (await screen.findByRole('heading', { name: 'Status' })).closest('section')!;
    expect(within(status as HTMLElement).getByText('/ping')).toBeTruthy();
    expect(within(status as HTMLElement).getByText('no key')).toBeTruthy();
    const proposals = screen.getByRole('heading', { name: 'Proposals' }).closest('section')!;
    expect(within(proposals as HTMLElement).getByText('operate')).toBeTruthy();
  });

  it('marks the contract prose as English and its code as code', async () => {
    show();
    const code = await screen.findByText('X-Api-Key');
    expect(code.tagName).toBe('CODE');
    expect(code.closest('[lang="en"]')).not.toBeNull();
  });

  it('narrows the operations to what the search names', async () => {
    const user = userEvent.setup();
    show();
    await user.type(await screen.findByLabelText('Search the operations'), 'apply');

    expect(screen.queryByText('/ping')).toBeNull();
    expect(screen.getByText('/decisions/apply')).toBeTruthy();
    await user.clear(screen.getByLabelText('Search the operations'));
    await user.type(screen.getByLabelText('Search the operations'), 'nothing-like-it');
    expect(screen.getByText('No operation matches.')).toBeTruthy();
  });

  it('shows how to call an operation, the key included', async () => {
    show();
    await screen.findByText('/decisions/apply');
    const example = document.querySelector('.api-example')?.textContent ?? '';
    expect(example).toContain('/api/v1/ping');
    expect(document.body.textContent).toContain('X-Api-Key: $ROUTARR_KEY');
  });
});
