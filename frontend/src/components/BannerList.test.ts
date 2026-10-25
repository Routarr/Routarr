import { describe, it, expect } from 'vitest';
import { screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import BannerList from './BannerList.svelte';

const NO_BREAK = ' ';

describe('BannerList', () => {
  /** French sets a no-break space before the colon, Chinese a full-width colon. */
  it('joins what a finding is about to it as the language writes the colon', () => {
    renderWithI18n(BannerList, {
      props: { tone: 'warning', items: [{ lead: 'Film', text: 'raison' }] },
      language: 'fr',
      strings: { FilterTerm: `{label}${NO_BREAK}: {value}` },
    });

    expect(screen.getByRole('listitem').textContent?.trim()).toBe(`Film${NO_BREAK}: raison`);
    expect(screen.getByText('Film', { exact: false, selector: 'strong' })).toBeTruthy();
  });

  it('sets a note and the screen that settles a finding after it, a space apart', () => {
    renderWithI18n(BannerList, {
      props: {
        tone: 'warning',
        items: [
          { text: 'No mapping.', note: 'Also a step.', link: { to: '/categories', label: 'Open' } },
        ],
      },
    });

    expect(screen.getByRole('listitem').textContent).toBe('No mapping. Also a step. Open');
    expect(screen.getByRole('link', { name: 'Open' })).toHaveAttribute('href', '/categories');
  });
});
