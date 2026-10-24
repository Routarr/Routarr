import { describe, it, expect, vi, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import { ApiError, api } from '../api/client';
import type { EnrichmentReport, MetadataProvider, MetadataProviders } from '../api/types';
import SourceRefresh from './SourceRefresh.svelte';
import { publishAccess } from '../lib/access.svelte';

function provider(id: string, over: Partial<MetadataProvider> = {}): MetadataProvider {
  return {
    id,
    display_name: id.toUpperCase(),
    fetched: true,
    needs_key: false,
    key_env: null,
    configured: true,
    fields: [],
    media_types: ['movie', 'series'],
    website: null,
    ...over,
  };
}

const CATALOGUE: MetadataProviders = {
  providers: [
    provider('arr', { fetched: false }),
    provider('tmdb'),
    provider('omdb', { needs_key: true, configured: false }),
    provider('tenrai'),
  ],
  order: ['arr', 'tmdb', 'omdb'],
};

function report(over: Partial<EnrichmentReport> = {}): EnrichmentReport {
  return { searched: 0, considered: 3, enriched: 3, failed: 0, skipped: 0, deferred: 0, ...over };
}

function show(catalogue: MetadataProviders = CATALOGUE) {
  vi.spyOn(api, 'getSources').mockResolvedValue(catalogue);
  return renderWithI18n(SourceRefresh);
}

afterEach(() => vi.restoreAllMocks());

/**
 * A source's answers read again on demand: offered for each source a pass
 * asks, and only those, since a pass reads the saved order and nothing else.
 */
describe('SourceRefresh', () => {
  it('offers each source a pass asks, and no other', async () => {
    show();

    expect(await screen.findByRole('button', { name: 'Read again – TMDB' })).toBeTruthy();
    // The library's own metadata, a source waiting for its key, one switched off.
    for (const name of ['ARR', 'OMDB', 'TENRAI']) {
      expect(screen.queryByRole('button', { name: `Read again – ${name}` })).toBeNull();
    }
  });

  it('reads a source again and says what that did', async () => {
    const asked = vi.spyOn(api, 'refreshSource').mockResolvedValue(report());
    show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Read again – TMDB' }));

    expect(await screen.findByText('TMDB: answers read again: 3, failed: 0')).toBeTruthy();
    expect(asked).toHaveBeenCalledWith('tmdb');
  });

  it('says what was read when part of it failed', async () => {
    vi.spyOn(api, 'refreshSource').mockResolvedValue(report({ enriched: 2, failed: 1 }));
    show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Read again – TMDB' }));

    // A warning, not a success: part of what was asked was not done.
    const said = await screen.findByText('TMDB: answers read again: 2, failed: 1');
    expect(said.closest('.banner-warning')).toBeTruthy();
  });

  it('announces a refresh that was refused', async () => {
    vi.spyOn(api, 'refreshSource').mockRejectedValue(
      new ApiError('a pass is running', 409, 'conflict'),
    );
    show();

    await fireEvent.click(await screen.findByRole('button', { name: 'Read again – TMDB' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('a pass is running');
  });

  it('says so when no source can be read again', async () => {
    show({ ...CATALOGUE, order: ['arr'] });

    expect(
      await screen.findByText(
        "No source to read again: Radarr and Sonarr's metadata comes with each sync.",
      ),
    ).toBeTruthy();
  });
});

describe('for a reader', () => {
  /** A pass reads every answer again and writes them, which a reader is refused. */
  it('offers the owner a read again per source, and a reader the list alone', async () => {
    const { unmount } = show();
    expect(await screen.findByRole('button', { name: 'Read again – TMDB' })).toBeInTheDocument();
    unmount();

    publishAccess(true);
    show();

    expect(await screen.findByText('TMDB')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Read again/ })).toBeNull();
  });
});
