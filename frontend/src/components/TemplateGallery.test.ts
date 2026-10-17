import { describe, it, expect, vi } from 'vitest';
import { screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { renderWithI18n } from '../test/render';
import TemplateGallery from './TemplateGallery.svelte';
import { template } from '../test/fixtures';

describe('TemplateGallery', () => {
  it('names each template on the action it offers, and hands the chosen one over', async () => {
    const onChoose = vi.fn();
    const anime = template({ id: 'anime', name: 'Anime', category: 'anime', added_version: 1 });
    renderWithI18n(TemplateGallery, { props: { templates: [template(), anime], onChoose } });

    expect(screen.getByRole('heading', { name: 'Documentaries', level: 3 })).toBeTruthy();
    const tile = screen.getByRole('heading', { name: 'Documentaries' }).closest('li');
    expect(tile?.querySelector('.template-target')?.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      'Category documentaries',
    );
    expect(screen.getByRole('button', { name: 'Add – Documentaries' })).toBeTruthy();
    expect(screen.getByText('Added')).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Replace – Anime' }));

    expect(onChoose).toHaveBeenCalledWith(expect.objectContaining({ id: 'anime' }));
  });

  it('says which source a template needs, and where to set it up', () => {
    const needs = [{ id: 'tmdb', name: 'TMDB' }];
    renderWithI18n(TemplateGallery, {
      props: { templates: [template({ needs })], onChoose: vi.fn() },
    });

    expect(screen.getByText(/Source needed, not set up yet: TMDB/)).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Set up the sources' }).getAttribute('href')).toBe(
      '/sources',
    );
  });

  it('says on its card that the requester template has no one to route yet', () => {
    const requesters = template({ id: 'requesters', parameter: 'requesters', category: null });
    renderWithI18n(TemplateGallery, { props: { templates: [requesters], onChoose: vi.fn() } });

    expect(screen.getByText('No Seerr request found in your library yet.')).toBeTruthy();
  });

  it('offers to update the requester template once added, person by person', () => {
    const requesters = template({
      id: 'requesters',
      name: 'Seerr requests by person',
      parameter: 'requesters',
      category: null,
      added_version: 1,
    });
    renderWithI18n(TemplateGallery, { props: { templates: [requesters], onChoose: vi.fn() } });

    expect(screen.getByRole('button', { name: 'Update – Seerr requests by person' })).toBeTruthy();
  });
});
