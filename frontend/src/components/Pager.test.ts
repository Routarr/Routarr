import { describe, it, expect } from 'vitest';
import { fireEvent, screen } from '@testing-library/svelte';

import { renderWithI18n } from '../test/render';
import PagerHarness from '../test/PagerHarness.svelte';

/**
 * The pages of a list the server pages, under its table. The screen asks for
 * the page the pager binds, so a press that moves nothing, or moves past an
 * end, is a request for a page that is not there.
 */

const STRINGS = {
  PageOf: 'Page {page} of {total}',
  ItemCount: 'Items: {count}',
  Previous: 'Previous',
  Next: 'Next',
};

const pages = (page: number, total_pages: number) => ({
  page,
  per_page: 50,
  total: total_pages * 50,
  total_pages,
});

const show = (pagination: ReturnType<typeof pages> | undefined) =>
  renderWithI18n(PagerHarness, { props: { pagination }, strings: STRINGS });

describe('Pager', () => {
  /** Two buttons that can do nothing are noise under a table that fits on one page. */
  it('draws nothing for a list that fits on one page, or before it loads', () => {
    show(pages(1, 1));
    expect(screen.queryByRole('button', { name: 'Next' })).toBeNull();

    show(undefined);
    expect(screen.queryByRole('button', { name: 'Next' })).toBeNull();
  });

  it('says where the reader is and how long the whole list is, with the count the screen names', () => {
    show(pages(1, 3));

    expect(screen.getByText('Page 1 of 3 · Items: 150')).toBeTruthy();
  });

  it('moves the page it binds one step at a time, and no further than either end', async () => {
    const { rerender } = show(pages(1, 2));
    const previous = screen.getByRole('button', { name: 'Previous' }) as HTMLButtonElement;
    const next = screen.getByRole('button', { name: 'Next' }) as HTMLButtonElement;
    expect(previous.disabled).toBe(true);

    await fireEvent.click(next);
    expect(screen.getByTestId('page')).toHaveTextContent('2');
    await rerender({ pagination: pages(2, 2) });
    expect(next.disabled).toBe(true);

    await fireEvent.click(previous);
    expect(screen.getByTestId('page')).toHaveTextContent('1');
  });

  /**
   * At the last page the pressed Next turns disabled, which drops the focus
   * to the page's start: it goes to Previous, where the reader can go on.
   */
  it('hands the focus to the other button when the pressed one reaches an end', async () => {
    show(pages(1, 2));
    const previous = screen.getByRole('button', { name: 'Previous' });
    const next = screen.getByRole('button', { name: 'Next' });
    next.focus();

    await fireEvent.click(next);
    await new Promise((resolve) => setTimeout(resolve));

    expect(screen.getByTestId('page')).toHaveTextContent('2');
    expect(document.activeElement).toBe(previous);
  });
});
