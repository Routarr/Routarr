import { describe, it, expect, vi } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { MetadataProvider } from '../api/types';
import ProviderOrder from './ProviderOrder.svelte';

/**
 * A source's credential lives in the source's own row: a key is not a setting
 * of the application, it is a property of the source it unlocks. Stated below
 * the list, enabling TMDb would mean scrolling past the whole thing, saving,
 * scrolling back and saving again.
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
  FacetGenres: 'Genres',
  FacetKeywords: 'Keywords',
  FacetLanguages: 'Original languages',
  ListSeparator: ' ; ',
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

/** What enabling a source brings, in the reader's words rather than the engine's identifiers. */
it('names the fields a source supplies by their captions', async () => {
  renderWithI18n(ProviderOrder, {
    props: {
      id: 'sources',
      catalogue: [
        provider({
          needs_key: false,
          configured: true,
          fields: ['genres', 'keywords', 'original_language'],
        }),
      ],
      value: 'tmdb',
      onChange: vi.fn(),
      keys: {},
      onKeyChange: vi.fn(),
    },
    strings: STRINGS,
  });

  expect(
    await screen.findByText(/No key needed · Genres ; Keywords ; Original languages/),
  ).toBeTruthy();
});

describe('a credential is edited in the row of the source it unlocks', () => {
  /**
   * The row reports the edit and does not reach into the parent's draft.
   *
   * `keys` is a plain record, not a `$bindable`, so writing through it is an
   * ownership violation Svelte flags in development, and it works only when
   * the caller passes a `$state` proxy. A caller passing an ordinary object
   * would write into nothing, silently, and the key would never be saved.
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
   * Rendered on `keys` alone, with the write going through an optional
   * callback, the row would give a caller passing the record without
   * `onKeyChange` a field that accepts a key and drops it: the silent loss the
   * callback exists to remove, moved one step along.
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

/** A row's buttons name their source, so a list of buttons tells them apart. */
describe('each source button names its source', () => {
  it('names every button of a row with the source it acts on', async () => {
    renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [
          provider({
            id: 'arr',
            display_name: 'Radarr / Sonarr',
            needs_key: false,
            fetched: false,
          }),
          provider({ id: 'anilist', display_name: 'AniList', needs_key: false, configured: true }),
          provider(),
        ],
        value: 'arr,anilist',
        onChange: vi.fn(),
      },
      strings: STRINGS,
    });

    expect(await screen.findByRole('button', { name: 'Enable – TMDb' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Disable – AniList' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Move up – AniList' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Move down – Radarr / Sonarr' })).toBeTruthy();
  });
});

/**
 * A press that moves a row draws it again, in its new place or in the other
 * list, and the pressed button with it. The focus follows the source rather
 * than falling to the page, on the guide's own metadata step.
 */
describe('a source keeps the focus when its row moves', () => {
  function showSources(value: string) {
    const view = renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [
          provider({
            id: 'arr',
            display_name: 'Radarr / Sonarr',
            needs_key: false,
            fetched: false,
          }),
          provider({ id: 'anilist', display_name: 'AniList', needs_key: false }),
          provider(),
        ],
        value,
        onChange: (next: string) => void view.rerender({ value: next }),
        keys: { tmdb_api_key: '' },
        onKeyChange: vi.fn(),
      },
      strings: STRINGS,
    });
  }

  it('enabling a source leaves the focus on that source', async () => {
    showSources('arr');

    await userEvent.click(await screen.findByRole('button', { name: 'Enable – AniList' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Disable – AniList' }),
      ),
    );
  });

  it('disabling a source leaves the focus on that source', async () => {
    showSources('arr,anilist');

    await userEvent.click(await screen.findByRole('button', { name: 'Disable – AniList' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Enable – AniList' })),
    );
  });

  /** Its Enable refused until a key is given, the source's key field is what is left. */
  it('hands a source switched off without its key to that key field', async () => {
    showSources('arr,tmdb');

    await userEvent.click(await screen.findByRole('button', { name: 'Disable – TMDb' }));

    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText('TMDb')));
  });

  /** At the top, its Move up refused, the row keeps the focus on the way back down. */
  it('moving a source keeps the focus on its row', async () => {
    showSources('arr,anilist');

    await userEvent.click(await screen.findByRole('button', { name: 'Move up – AniList' }));

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: 'Move down – AniList' }),
      ),
    );
  });
});
