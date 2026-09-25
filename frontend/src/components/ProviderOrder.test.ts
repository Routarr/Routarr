import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { MetadataProvider } from '../api/types';
import ProviderOrder from './ProviderOrder.svelte';

/**
 * A source's credential lives in the source's own row: a key is not a setting
 * of the application, it is a property of the source it unlocks. Stated three
 * blocks below the list, enabling TMDb meant scrolling past the whole thing,
 * saving, scrolling back and saving again.
 */

const STRINGS = {
  SourcesActive: 'Active, in priority order',
  SourcesInactive: 'Inactive',
  DisableSource: 'Disable',
  EnableSource: 'Enable',
  MoveUp: 'Move up',
  MoveDown: 'Move down',
  ProviderKeyPlaceholder: 'API key',
  ProviderKeyOrEnv: 'API key or {variable}',
  SecretConfiguredPlaceholder: 'A key is stored – type to replace it',
  ProviderNeedsKey: 'Needs a key',
  ProviderNoKeyNeeded: 'No key needed',
  ProviderInactive: 'inactive',
};

function provider(over: Partial<MetadataProvider> = {}): MetadataProvider {
  return {
    id: 'tmdb',
    display_name: 'TMDb',
    fetched: true,
    needs_key: true,
    key_env: null,
    configured: false,
    fields: ['genres'],
    ...over,
  } as MetadataProvider;
}

describe('a credential is edited in the row of the source it unlocks', () => {
  /**
   * The row reports the edit; it does not reach into the parent's draft.
   *
   * `keys` is a plain record, not a `$bindable`, so writing through it is an
   * ownership violation Svelte flags in development — and it worked only
   * because the one caller happened to pass a `$state` proxy. A second caller
   * passing an ordinary object would have written into nothing, silently, and
   * the key would never have been saved.
   */
  it('reports the key through its callback without writing the record it was given', async () => {
    const onKeyChange = vi.fn();
    // Frozen, so a write cannot pass unnoticed: assignment to a frozen object
    // throws in a module, which every component here is.
    const keys = Object.freeze({ tmdb_api_key: '' });

    renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [provider()],
        value: 'tmdb',
        onChange: vi.fn(),
        keys,
        onKeyChange,
      },
      strings: STRINGS,
    });

    await userEvent.type(await screen.findByLabelText('TMDb'), 'ab');

    expect(onKeyChange.mock.calls).toEqual([
      ['tmdb_api_key', 'a'],
      ['tmdb_api_key', 'ab'],
    ]);
    expect(keys.tmdb_api_key).toBe('');
  });

  /**
   * A field is offered only where it can be written.
   *
   * The row rendered on `keys` alone while the write went through an optional
   * `onKeyChange?.(…)`, so a caller passing the record without the callback got
   * a field that accepted a key and dropped it — the same silent loss the
   * callback was introduced to remove, moved one step along.
   */
  it('offers no credential field it has no way to report', async () => {
    renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [provider()],
        value: 'tmdb',
        onChange: vi.fn(),
        keys: { tmdb_api_key: '' },
        // No `onKeyChange`.
      },
      strings: STRINGS,
    });

    await screen.findByText('TMDb');
    expect(screen.queryByLabelText('TMDb')).toBeNull();
  });

  /**
   * The field is rendered whether or not the source is switched on and whether
   * or not a key is stored: it is the only field for it anywhere, so hiding it
   * once the source works leaves a leaked key impossible to rotate. The
   * placeholder is what says which of the two situations the reader is in.
   */
  it('offers the field for a source that already has a key', async () => {
    renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [provider({ configured: true })],
        value: 'tmdb',
        onChange: vi.fn(),
        keys: { tmdb_api_key: '' },
        onKeyChange: vi.fn(),
      },
      strings: STRINGS,
    });

    const field = await screen.findByLabelText('TMDb');
    expect(field.getAttribute('placeholder')).toBe('A key is stored – type to replace it');
  });
});
