import { describe, it, expect, vi } from 'vitest';
import { screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';

import { renderWithI18n } from '../test/render';
import type { MetadataProvider } from '../api/types';
import ProviderOrder from './ProviderOrder.svelte';

function provider(over: Partial<MetadataProvider> = {}): MetadataProvider {
  return {
    id: 'tmdb',
    display_name: 'TMDB',
    fetched: true,
    needs_key: true,
    key_env: null,
    configured: false,
    fields: ['genres'],
    media_types: ['movie', 'series'],
    ...over,
  } as MetadataProvider;
}

it('names the fields a source supplies by their captions, and a source for series alone', async () => {
  renderWithI18n(ProviderOrder, {
    props: {
      id: 'sources',
      catalogue: [
        provider({
          needs_key: false,
          configured: true,
          fields: ['genres', 'keywords', 'original_language'],
        }),
        provider({ id: 'tvdb', display_name: 'TheTVDB', media_types: ['series'] }),
      ],
      value: 'tmdb,tvdb',
      onChange: vi.fn(),
      keys: {},
      onKeyChange: vi.fn(),
    },
  });

  const tmdb = await screen.findByText(/no key needed · Genres, Keywords, Original languages/);
  expect(tmdb.textContent).not.toContain('series only');
  expect(screen.getByText(/· series only/)).toBeTruthy();
});

describe('a credential is edited in the row of the source it unlocks', () => {
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
    });

    await userEvent.type(await screen.findByLabelText('TMDB'), 'ab');

    expect(onKeyChange.mock.calls).toEqual([
      ['tmdb_api_key', 'a'],
      ['tmdb_api_key', 'ab'],
    ]);
    expect(keys.tmdb_api_key).toBe('');
  });

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
    });

    await screen.findByText('TMDB');
    expect(screen.queryByLabelText('TMDB')).toBeNull();
  });

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
    });

    const field = await screen.findByLabelText('TMDB');
    expect(field.getAttribute('placeholder')).toBe('A key is stored. Type to replace it');
  });

  /** A user-supported TheTVDB key also needs its subscriber's PIN. */
  it("offers TheTVDB's subscriber PIN under its key, masked and explained", async () => {
    const onKeyChange = vi.fn();
    renderWithI18n(ProviderOrder, {
      props: {
        id: 'sources',
        catalogue: [provider({ id: 'tvdb', display_name: 'TheTVDB' }), provider()],
        value: 'tvdb,tmdb',
        onChange: vi.fn(),
        keys: { tvdb_api_key: '', tvdb_subscriber_pin: '', tmdb_api_key: '' },
        onKeyChange,
      },
    });

    const pin = await screen.findByLabelText('TheTVDB subscriber PIN');
    expect(pin.getAttribute('type')).toBe('password');
    expect(pin).toHaveAccessibleDescription(/Only a user-supported key needs it/);
    expect(screen.getAllByLabelText(/subscriber PIN/)).toHaveLength(1);

    await userEvent.type(pin, '42');
    expect(onKeyChange).toHaveBeenLastCalledWith('tvdb_subscriber_pin', '42');
  });
});

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
    });

    expect(await screen.findByRole('button', { name: 'Enable – TMDB' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Disable – AniList' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Move up – AniList' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Move down – Radarr / Sonarr' })).toBeTruthy();
  });
});

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

  it('hands a source switched off without its key to that key field', async () => {
    showSources('arr,tmdb');

    await userEvent.click(await screen.findByRole('button', { name: 'Disable – TMDB' }));

    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText('TMDB')));
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
