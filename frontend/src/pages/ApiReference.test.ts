import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import { api } from '../api/client';
import type { OpenApiDocument } from '../api/openapi';
import ApiReference from './ApiReference.svelte';
import { publishAccess } from '../lib/access.svelte';

const DOC: OpenApiDocument = {
  info: { title: 'Routarr API', version: '1', description: 'A long `introduction`.' },
  servers: [{ url: '/api/v1' }],
  tags: [
    { name: 'status', description: 'Whether Routarr is up.' },
    { name: 'decisions', description: 'Applying.' },
  ],
  paths: {
    '/ping': { get: { operationId: 'ping', summary: 'Whether Routarr is up', tags: ['status'] } },
    '/decisions/apply': {
      post: {
        operationId: 'apply',
        summary: 'Apply chosen decisions',
        tags: ['decisions'],
        'x-routarr-scope': 'operate',
        parameters: [
          {
            name: 'Prefer',
            in: 'header',
            required: false,
            description: 'Send `respond-async` to be answered at once.',
          },
        ],
        responses: { '202': { description: 'Started, with the job in `Location`.' } },
      },
    },
    '/proposals/apply': {
      post: {
        operationId: 'apply_deprecated',
        summary: 'Apply chosen decisions',
        tags: ['decisions'],
        'x-routarr-scope': 'operate',
        deprecated: true,
      },
    },
  },
  components: {
    schemas: {
      Pong: {
        type: 'object',
        properties: { status: { type: 'string', description: 'Always `ok`.' } },
      },
    },
  },
};

function show() {
  vi.spyOn(api, 'openApi').mockResolvedValue(DOC);
  return renderWithI18n(ApiReference);
}

afterEach(() => vi.restoreAllMocks());

describe('ApiReference', () => {
  it('groups the operations and names the scope each asks, or none', async () => {
    show();

    const status = (await screen.findByRole('heading', { name: 'Status' })).closest('section')!;
    expect(within(status as HTMLElement).getByText('/ping')).toBeTruthy();
    expect(within(status as HTMLElement).getByText('no key')).toBeTruthy();
    const decisions = screen.getByRole('heading', { name: 'Decisions' }).closest('section')!;
    expect(within(decisions as HTMLElement).getByText('Operate')).toBeTruthy();
  });

  it('lists an old path once, beside its new one, and never among the operations', async () => {
    show();
    const table = await screen.findByRole('table', { name: 'Deprecated paths' });
    const row = within(table).getAllByRole('row')[1]!;
    expect(row).toHaveTextContent('POST /proposals/apply');
    expect(row).toHaveTextContent('POST /decisions/apply');
    const decisions = screen.getByRole('heading', { name: 'Decisions' }).closest('section')!;
    expect(within(decisions).queryByText('/proposals/apply')).toBeNull();
  });

  it('marks the contract prose as English and its code as code', async () => {
    show();
    const code = await screen.findByText('respond-async');
    expect(code.tagName).toBe('CODE');
    expect(code.closest('[lang="en"]')).not.toBeNull();
  });

  it('leaves the introduction to the document and points to where keys are made', async () => {
    show();
    await screen.findByText('/ping');

    expect(screen.queryByText(/A long/)).toBeNull();
    const link = screen.getByRole('link', { name: 'Applications' });
    expect(link).toHaveAttribute('href', '/applications');
    expect(link.closest('p')).toHaveTextContent(
      'Each call sends a key made on the Applications screen, in its X-Api-Key header.',
    );
  });

  it('lays the English prose out left to right whatever the page direction', async () => {
    const { container } = show();
    await screen.findByText('respond-async');

    const english = [...container.querySelectorAll('[lang="en"]')];
    expect(english.length).toBeGreaterThan(2);
    for (const element of english) expect(element.getAttribute('dir')).toBe('ltr');
  });

  it('marks code in every description, not only in the operation text', async () => {
    const { container } = show();
    await screen.findByText('respond-async');

    for (const code of ['respond-async', 'Location', 'ok']) {
      expect(screen.getByText(code).tagName).toBe('CODE');
    }
    const shown = [...container.querySelectorAll('td, .api-responses li')].map(
      (node) => node.textContent ?? '',
    );
    expect(shown.some((text) => text.includes('`'))).toBe(false);
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

  it('lets a keyboard reach the end of an example', async () => {
    show();
    await fireEvent.click(await screen.findByText('/decisions/apply'));

    const example = screen.getByRole('region', { name: 'Example – POST /decisions/apply' });
    expect(example).toHaveAttribute('tabindex', '0');
    expect(example).toHaveClass('api-example');
  });
});

describe('for a reader', () => {
  it('links the owner to the screen that makes keys, and names it to a reader', async () => {
    const { unmount } = show();
    expect(await screen.findByRole('link', { name: 'Applications' })).toBeInTheDocument();
    unmount();

    publishAccess(true);
    show();

    const line = await screen.findByText(/Each call sends a key made on the/);
    expect(line).toHaveTextContent(
      'Each call sends a key made on the Applications screen, in its X-Api-Key header.',
    );
    expect(screen.queryByRole('link', { name: 'Applications' })).toBeNull();
  });
});
