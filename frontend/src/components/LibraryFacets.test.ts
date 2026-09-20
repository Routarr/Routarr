import { describe, it, expect, afterEach } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import LibraryFacets from './LibraryFacets.svelte';

/**
 * Reference data, on the screen whose subject is something else.
 *
 * The panel answers "what does my library actually hold" while a rule is being
 * written, and it sat open above the rule table: 428px of a 1440px screen, 76%
 * of a 768px one, and a screen and a half of a phone — so the rules screen
 * opened on everything except the rules.
 */

const STRINGS = {
  InYourLibrary: 'In your library ({count})',
  FacetGenres: 'Genres',
  FacetLanguages: 'Languages',
  FacetCountries: 'Origin countries',
  FacetCertifications: 'Certifications',
  FacetTags: 'Tags',
  FacetSeriesTypes: 'Series types',
  FacetRootFolders: 'Root folders',
  FacetsEmpty: 'Nothing counted yet.',
  FacetsMore: 'and {count} more',
  FacetsNoMetadata: 'Without metadata: {count}',
};

const facets = {
  total_media: 38,
  vocabularies: {
    original_languages: [{ value: 'ja', label: 'Japanese (ja)', count: 0 }],
    origin_countries: [],
  },
  without_metadata: 0,
  genres: [
    { value: 'Animation', count: 12 },
    { value: 'Music', count: 3 },
  ],
  original_languages: [{ value: 'ja', count: 9 }],
  origin_countries: [],
  certifications: [],
  tags: [],
  series_types: [],
  root_folders: [{ value: '/movies/standard', count: 30 }],
};

const show = () => renderWithI18n(LibraryFacets, { props: { facets }, strings: STRINGS }).container;

/**
 * Open it the way a browser does.
 *
 * jsdom flips the `open` attribute when the summary is clicked but does not
 * emit the `toggle` event that Svelte binds to, so a bare click changes the
 * DOM and leaves the component's own state behind.
 */
async function disclose(container: HTMLElement) {
  const details = container.querySelector('details')!;
  await fireEvent.click(container.querySelector('summary')!);
  await fireEvent(details, new Event('toggle'));
}

afterEach(() => {
  try {
    localStorage.removeItem('routarr.facetsOpen');
  } catch {
    // jsdom always has it; a browser told to block site data would not.
  }
});

describe('LibraryFacets', () => {
  it('is folded on arrival, and says what it is holding', () => {
    const container = show();

    const panel = container.querySelector('details');
    expect(panel?.hasAttribute('open')).toBe(false);
    // Folded is not hidden: the heading still carries the size of the library,
    // which is the figure that says whether the panel is worth opening.
    expect(screen.getByText('In your library (38)')).toBeInTheDocument();
  });

  it('shows the axes once it is opened', async () => {
    const container = show();

    await disclose(container);

    expect(container.querySelector('details')?.hasAttribute('open')).toBe(true);
    expect(screen.getByText('Genres')).toBeInTheDocument();
    // Named from the vocabulary: the counts say `ja`, and only the vocabulary
    // knows that is Japanese.
    expect(screen.getByText('Japanese (ja)')).toBeInTheDocument();
    // Only the axes carrying something: an empty card teaches nothing.
    expect(screen.queryByText('Certifications')).toBeNull();
  });

  /**
   * The preference is per viewer and survives the navigation, or the panel
   * folds itself again on every visit to the screen it belongs to.
   */
  it('remembers that it was opened', async () => {
    const first = show();
    await disclose(first);
    expect(localStorage.getItem('routarr.facetsOpen')).toBe('1');

    const second = show();
    expect(second.querySelector('details')?.hasAttribute('open')).toBe(true);
  });
});
